// Package main implements cadence-replay-helper, which re-executes a
// historical Flow transaction with the network's own FVM against the register
// state an access node serves for the parent block, and emits the Cadence
// interpreter's statement stream as NDJSON on stdout.
//
// It is a separate module from go-helper because the FVM must be the version
// the network ran: flow-go is pinned here to the release mainnet's nodes report
// (`/v1/node_version_info`), and that pins Cadence, while `record` mode stays
// on its own Cadence pin.
//
// Usage:
//
//	cadence-replay-helper --tx-id <hex> [--access-node host:port]
//	    [--chain flow-mainnet] --source-dir <dir> [--verdict-out <file>]
//
// Prestate is the parent block's register state plus the effects of every
// transaction that precedes the target in its block, re-executed in order
// (the `replay-preceding` strategy). The target's own statement stream is
// the only one traced.
//
// The NDJSON vocabulary is the one go-helper emits and the Rust recorder
// parses (`step`, `variable`, `call`, `return`, `event`, `error`) plus the
// resource lifecycle records (`resource_create`, `resource_move`,
// `resource_destroy`). The verdict -- whether the replay reproduced the
// on-chain result -- is written as JSON to --verdict-out, because it is a
// fact about the run rather than an event in it.
package main

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"

	"github.com/onflow/cadence"
	"github.com/onflow/cadence/ast"
	"github.com/onflow/cadence/common"
	"github.com/onflow/cadence/encoding/ccf"
	"github.com/onflow/cadence/interpreter"
	"github.com/onflow/cadence/runtime"
	"github.com/onflow/cadence/sema"
	sdk "github.com/onflow/flow-go-sdk"
	"github.com/rs/zerolog"

	"github.com/onflow/flow-go/fvm"
	reusableRuntime "github.com/onflow/flow-go/fvm/runtime"
	"github.com/onflow/flow-go/model/flow"
	"github.com/onflow/flow-go/utils/debug"
)

const defaultAccessNode = "access.mainnet.nodes.onflow.org:9000"

// TraceEvent is one NDJSON line. Field names match go-helper's and the Rust
// side's serde renames.
type TraceEvent struct {
	Type         string     `json:"type"`
	File         string     `json:"file,omitempty"`
	Line         int        `json:"line,omitempty"`
	Column       int        `json:"column,omitempty"`
	Name         string     `json:"name,omitempty"`
	Value        string     `json:"value,omitempty"`
	Payload      string     `json:"payload,omitempty"`
	CadenceType  string     `json:"cadence_type,omitempty"`
	Message      string     `json:"message,omitempty"`
	Args         []TraceArg `json:"args,omitempty"`
	ResourceType string     `json:"resource_type,omitempty"`
	UUID         *uint64    `json:"uuid,omitempty"`
	Owner        string     `json:"owner,omitempty"`
	FromOwner    string     `json:"from_owner,omitempty"`
	ToOwner      string     `json:"to_owner,omitempty"`
}

type TraceArg struct {
	Name        string `json:"name"`
	Value       string `json:"value"`
	CadenceType string `json:"cadence_type,omitempty"`
}

// Verdict is the --verdict-out document: what ran, against what, and whether
// the replay reproduced what the chain recorded.
type Verdict struct {
	Schema            string      `json:"schema"`
	TransactionID     string      `json:"transactionId"`
	BlockID           string      `json:"blockId"`
	BlockHeight       uint64      `json:"blockHeight"`
	IndexInBlock      int         `json:"indexInBlock"`
	PrecedingReplayed int         `json:"precedingReplayed"`
	PrestateHeight    uint64      `json:"prestateHeight"`
	PrestateStrategy  string      `json:"prestateStrategy"`
	FlowGoVersion     string      `json:"flowGoVersion"`
	Proposer          string      `json:"proposer"`
	Payer             string      `json:"payer"`
	Authorizers       []string    `json:"authorizers"`
	OnChain           ResultFacts `json:"onChain"`
	Replay            ResultFacts `json:"replay"`
	// Untraced is the same target on the same prestate with no debugger.
	// Traced and untraced computation differing means observing perturbed
	// the execution, which would make the trace a different run.
	Untraced             ResultFacts       `json:"untraced"`
	ObservationPerturbed bool              `json:"observationPerturbed"`
	EventsMatched        int               `json:"eventsMatched"`
	EventsCompared       int               `json:"eventsCompared"`
	FirstEventDiff       string            `json:"firstEventDiff,omitempty"`
	StatusMatched        bool              `json:"statusMatched"`
	Reproduced           bool              `json:"reproduced"`
	Steps                int               `json:"steps"`
	Sources              []SourceFact      `json:"sources"`
	MissingSources       []string          `json:"missingSources,omitempty"`
	PrecedingFailures    map[string]string `json:"precedingFailures,omitempty"`
}

type ResultFacts struct {
	Failed          bool     `json:"failed"`
	ErrorMessage    string   `json:"errorMessage,omitempty"`
	ComputationUsed uint64   `json:"computationUsed"`
	EventTypes      []string `json:"eventTypes"`
}

// SourceFact names one Cadence location the statement stream entered, the
// file it was written to, and the sha256 of the bytes written -- the code the
// replay executed, read from the same register state it executed against.
type SourceFact struct {
	Location string `json:"location"`
	File     string `json:"file"`
	SHA256   string `json:"sha256"`
	Origin   string `json:"origin"`
}

func main() {
	txIDFlag := flag.String("tx-id", "", "transaction id (hex)")
	accessNode := flag.String("access-node", defaultAccessNode, "access node gRPC address")
	chainFlag := flag.String("chain", "flow-mainnet", "chain id")
	sourceDir := flag.String("source-dir", "", "directory the executed Cadence sources are written to")
	verdictOut := flag.String("verdict-out", "", "file the replay verdict JSON is written to")
	computeLimit := flag.Uint64("compute-limit", flow.DefaultMaxTransactionGasLimit, "compute limit for the debugged transaction")
	fromInputs := flag.String("from-inputs", "", "replay from the transcript in this directory instead of the access node")
	recordInputs := flag.String("record-inputs", "", "write a transcript of this run's inputs to this directory")
	// The first argument may be the literal "replay", so the Rust side can
	// call either helper the same way.
	args := os.Args[1:]
	if len(args) > 0 && args[0] == "probe-floor" {
		probe := flag.NewFlagSet("probe-floor", flag.ExitOnError)
		node := probe.String("access-node", defaultAccessNode, "access node gRPC address")
		chain := probe.String("chain", "flow-mainnet", "chain id")
		out := probe.String("out", "", "file the probe JSON is written to (default stdout)")
		_ = probe.Parse(args[1:])
		if err := runProbeFloor(*node, flow.ChainID(*chain), *out); err != nil {
			fmt.Fprintf(os.Stderr, "cadence-replay-helper probe-floor: %v\n", err)
			os.Exit(1)
		}
		return
	}
	if len(args) > 0 && args[0] == "block" {
		fs := flag.NewFlagSet("block", flag.ExitOnError)
		node := fs.String("access-node", defaultAccessNode, "access node gRPC address")
		chain := fs.String("chain", "flow-mainnet", "chain id")
		height := fs.Uint64("height", 0, "block height")
		out := fs.String("out", "", "file the listing JSON is written to (default stdout)")
		_ = fs.Parse(args[1:])
		if err := runBlock(*node, flow.ChainID(*chain), *height, *out); err != nil {
			fmt.Fprintf(os.Stderr, "cadence-replay-helper block: %v\n", err)
			os.Exit(1)
		}
		return
	}
	if len(args) > 0 && args[0] == "replay" {
		args = args[1:]
	}
	if err := flag.CommandLine.Parse(args); err != nil {
		os.Exit(2)
	}
	if *txIDFlag == "" || *sourceDir == "" {
		fmt.Fprintln(os.Stderr, "usage: cadence-replay-helper replay --tx-id <hex> --source-dir <dir> [--access-node host:port] [--verdict-out file]")
		os.Exit(2)
	}

	err := run(*txIDFlag, *accessNode, flow.ChainID(*chainFlag), *sourceDir, *verdictOut, *fromInputs, *recordInputs, *computeLimit)
	if err != nil {
		fmt.Fprintf(os.Stderr, "cadence-replay-helper: %v\n", err)
		os.Exit(1)
	}
}

func run(rawTxID, accessNode string, chainID flow.ChainID, sourceDir, verdictOut, fromInputs, recordInputs string, computeLimit uint64) error {
	ctx := context.Background()
	chain := chainID.Chain()

	txID, err := flow.HexStringToIdentifier(strings.TrimPrefix(rawTxID, "0x"))
	if err != nil {
		return fmt.Errorf("bad transaction id %q: %w", rawTxID, err)
	}

	acq, err := acquire(ctx, txID, accessNode, chain, fromInputs)
	if err != nil {
		return err
	}
	defer acq.close()
	header, blockTxs := acq.header, acq.transactions
	blockID := header.ID()
	blockSnapshot := debug.NewCachingStorageSnapshot(acq.prestate)

	logger := zerolog.New(io.Discard)

	verdict := &Verdict{
		Schema:           "codetracer-flow-recorder/replay-verdict@1",
		TransactionID:    txID.String(),
		BlockID:          blockID.String(),
		BlockHeight:      header.Height,
		IndexInBlock:     -1,
		PrestateHeight:   header.Height - 1,
		PrestateStrategy: "replay-preceding",
		FlowGoVersion:    flowGoVersion,
	}

	var target *sdk.Transaction
	plain := newBlockRunner(chain, logger, header, networkOptions(chainID, computeLimit)...)
	for i, blockTx := range blockTxs {
		if flow.Identifier(blockTx.ID()) == txID {
			target = blockTx
			verdict.IndexInBlock = i
			break
		}
		result, err := plain.run(blockTx, i, blockSnapshot)
		if err != nil {
			return fmt.Errorf("replaying preceding transaction %s: %w", blockTx.ID(), err)
		}
		if result.Output.Err != nil {
			if verdict.PrecedingFailures == nil {
				verdict.PrecedingFailures = map[string]string{}
			}
			verdict.PrecedingFailures[blockTx.ID().String()] = result.Output.Err.Error()
		}
		for _, reg := range result.Snapshot.UpdatedRegisters() {
			blockSnapshot.Set(reg.Key, reg.Value)
		}
		verdict.PrecedingReplayed++
	}
	if target == nil {
		return fmt.Errorf("transaction %s is not among the %d transactions of block %s", txID, len(blockTxs), blockID)
	}

	verdict.Proposer = target.ProposalKey.Address.HexWithPrefix()
	verdict.Payer = target.Payer.HexWithPrefix()
	verdict.Authorizers = []string{}
	for _, a := range target.Authorizers {
		verdict.Authorizers = append(verdict.Authorizers, a.HexWithPrefix())
	}

	// The untraced control reads the same snapshot and writes nothing back
	// to it: a run's register updates are applied only by the loop above.
	untraced, err := plain.run(target, verdict.IndexInBlock, blockSnapshot)
	if err != nil {
		return fmt.Errorf("untraced control run: %w", err)
	}

	tracer := newStatementTracer(sourceDir, target, blockSnapshot)

	pool := reusableRuntime.NewCustomReusableCadenceRuntimePool(
		0,
		chain,
		runtime.Config{
			Debugger:                          tracer.debugger,
			ResourceOwnerChangeHandlerEnabled: true,
		},
		func(config runtime.Config) runtime.Runtime {
			return &observingRuntime{Runtime: runtime.NewRuntime(config), tracer: tracer}
		},
	)
	traced := newBlockRunner(
		chain,
		logger,
		header,
		append(networkOptions(chainID, computeLimit), fvm.WithReusableCadenceRuntimePool(pool))...,
	)

	type runOutcome struct {
		result debug.Result
		err    error
	}
	done := make(chan runOutcome, 1)
	tracer.debugger.RequestPause()
	go func() {
		defer func() {
			if r := recover(); r != nil {
				done <- runOutcome{err: fmt.Errorf("panic during traced execution: %v", r)}
			}
		}()
		result, err := traced.run(target, verdict.IndexInBlock, blockSnapshot)
		done <- runOutcome{result, err}
	}()

	var outcome runOutcome
loop:
	for {
		select {
		case stop := <-tracer.debugger.Stops():
			tracer.onStop(stop)
			tracer.debugger.RequestPause()
			tracer.debugger.Continue()
		case outcome = <-done:
			break loop
		}
	}
	if outcome.err != nil {
		return outcome.err
	}
	tracer.finish(outcome.result.Output.Err)

	verdict.Steps = tracer.steps
	verdict.Sources = tracer.sourceFacts()
	verdict.MissingSources = tracer.missingSources()
	verdict.Replay = replayFacts(outcome.result)
	verdict.Untraced = replayFacts(untraced)
	verdict.ObservationPerturbed = verdict.Untraced.ComputationUsed != verdict.Replay.ComputationUsed ||
		strings.Join(verdict.Untraced.EventTypes, ",") != strings.Join(verdict.Replay.EventTypes, ",")
	verdict.OnChain = onChainFacts(acq.onChain)
	compareResults(verdict, outcome.result, acq.onChain)
	if recordInputs != "" {
		if err := acq.save(recordInputs, txID); err != nil {
			return fmt.Errorf("recording inputs: %w", err)
		}
	}

	if err := tracer.writeEvents(os.Stdout); err != nil {
		return err
	}
	if verdictOut != "" {
		data, err := json.MarshalIndent(verdict, "", "  ")
		if err != nil {
			return err
		}
		if err := os.WriteFile(verdictOut, append(data, '\n'), 0o644); err != nil {
			return err
		}
	}
	return nil
}

// blockRunner executes transactions as an execution node does within one
// block: under the block's header and at each transaction's own index.
// flow-go's debug.RemoteDebugger runs every transaction at index 0, and the
// FVM partitions its UUID generator by index, so from the second
// transaction of a block on it hands out the first transaction's UUIDs --
// measured on mainnet as two transactions at index 1 whose events differed
// from the chain's in nothing but `proposalID` and `withdrawnUUID`.
type blockRunner struct {
	vm  fvm.VM
	ctx fvm.Context
}

func newBlockRunner(chain flow.Chain, logger zerolog.Logger, header *flow.Header, options ...fvm.Option) *blockRunner {
	// Signatures are not re-verified: the chain verified them, and the
	// replay has the transaction body without needing its keys' secrets.
	opts := append([]fvm.Option{
		fvm.WithLogger(logger),
		fvm.WithAuthorizationChecksEnabled(false),
		fvm.WithBlockHeader(header),
	}, options...)
	return &blockRunner{vm: fvm.NewVirtualMachine(), ctx: fvm.NewContext(chain, opts...)}
}

func (r *blockRunner) run(tx *sdk.Transaction, index int, snapshot debug.StorageSnapshot) (debug.Result, error) {
	builder := flow.NewTransactionBodyBuilder().
		SetScript(tx.Script).
		SetComputeLimit(tx.GasLimit).
		SetPayer(flow.Address(tx.Payer)).
		SetProposalKey(
			flow.Address(tx.ProposalKey.Address),
			tx.ProposalKey.KeyIndex,
			tx.ProposalKey.SequenceNumber,
		)
	for _, argument := range tx.Arguments {
		builder.AddArgument(argument)
	}
	for _, authorizer := range tx.Authorizers {
		builder.AddAuthorizer(flow.Address(authorizer))
	}
	body, err := builder.Build()
	if err != nil {
		return debug.Result{}, err
	}
	executionSnapshot, output, err := r.vm.Run(r.ctx, fvm.Transaction(body, uint32(index)), snapshot)
	if err != nil {
		return debug.Result{}, err
	}
	return debug.Result{Snapshot: executionSnapshot, Output: output}, nil
}

// networkOptions are the FVM options an execution node of chainID runs user
// transactions with (flow-go fvm/initialize.InitFvmOptions): the account
// storage limit always, and transaction fees on the public networks. Without
// fees the fee-deduction events and their computation are missing from the
// replay, and a payer who could not have paid would pass.
//
// Not reproduced: the node's block finder (Cadence `getBlock(at:)`) and its
// per-block entropy source (`revertibleRandom`). A transaction that uses
// either fails or diverges in the replay, and the verdict says which.
func networkOptions(chainID flow.ChainID, computeLimit uint64) []fvm.Option {
	opts := []fvm.Option{
		fvm.WithComputationLimit(computeLimit),
		fvm.WithAccountStorageLimit(true),
	}
	switch chainID {
	case flow.Mainnet, flow.Testnet, flow.Sandboxnet, flow.Previewnet:
		opts = append(opts, fvm.WithTransactionFeesEnabled(true))
	}
	return opts
}

// flowGoVersion is the flow-go release this helper's FVM is. It is stated in
// the verdict because a replay is only as faithful as that match.
const flowGoVersion = "v0.51.1"

func replayFacts(result debug.Result) ResultFacts {
	facts := ResultFacts{
		Failed:          result.Output.Err != nil,
		ComputationUsed: result.Output.ComputationUsed,
		EventTypes:      []string{},
	}
	if result.Output.Err != nil {
		facts.ErrorMessage = result.Output.Err.Error()
	}
	for _, ev := range result.Output.Events {
		facts.EventTypes = append(facts.EventTypes, string(ev.Type))
	}
	return facts
}

func onChainFacts(r OnChainResult) ResultFacts {
	facts := ResultFacts{
		Failed:          r.Failed,
		ErrorMessage:    r.ErrorMessage,
		ComputationUsed: r.ComputationUsed,
		EventTypes:      []string{},
	}
	for _, ev := range r.Events {
		facts.EventTypes = append(facts.EventTypes, ev.Type)
	}
	return facts
}

// compareResults decides `reproduced`: the same success or failure, and the
// same events with the same values, in the same order. Computation is
// recorded on both sides but not compared, because the debugger's own work
// is not metered identically and an exact match is not a property the chain
// promises across releases.
func compareResults(v *Verdict, replay debug.Result, onChain OnChainResult) {
	v.StatusMatched = v.Replay.Failed == v.OnChain.Failed
	n := len(onChain.Events)
	if len(replay.Output.Events) > n {
		n = len(replay.Output.Events)
	}
	v.EventsCompared = n
	for i := 0; i < n; i++ {
		if i >= len(onChain.Events) || i >= len(replay.Output.Events) {
			if v.FirstEventDiff == "" {
				v.FirstEventDiff = fmt.Sprintf("event %d: present on one side only (on-chain %d, replay %d)", i, len(onChain.Events), len(replay.Output.Events))
			}
			continue
		}
		want := onChain.Events[i]
		got := replay.Output.Events[i]
		gotValue, err := ccf.Decode(nil, got.Payload)
		if err != nil {
			if v.FirstEventDiff == "" {
				v.FirstEventDiff = fmt.Sprintf("event %d: replay payload does not decode: %v", i, err)
			}
			continue
		}
		if string(got.Type) != want.Type || gotValue.String() != want.Value {
			if v.FirstEventDiff == "" {
				v.FirstEventDiff = fmt.Sprintf("event %d: on-chain %s %s, replay %s %s", i, want.Type, want.Value, got.Type, gotValue.String())
			}
			continue
		}
		v.EventsMatched++
	}
	v.Reproduced = v.StatusMatched && v.EventsMatched == v.EventsCompared
}

// observingRuntime wraps the Cadence runtime so the interface every executor
// is given reports resource creation and ownership changes to the tracer. The
// FVM's own ResourceOwnerChanged is a no-op, so nothing is lost by
// intercepting it, and GenerateUUID is forwarded unchanged.
type observingRuntime struct {
	runtime.Runtime
	tracer *statementTracer
}

func (r *observingRuntime) wrap(c runtime.Context) runtime.Context {
	c.Interface = &observingInterface{Interface: c.Interface, tracer: r.tracer}
	return c
}

func (r *observingRuntime) NewTransactionExecutor(script runtime.Script, c runtime.Context) runtime.Executor {
	return r.Runtime.NewTransactionExecutor(script, r.wrap(c))
}

func (r *observingRuntime) ExecuteTransaction(script runtime.Script, c runtime.Context) error {
	return r.Runtime.ExecuteTransaction(script, r.wrap(c))
}

func (r *observingRuntime) NewContractFunctionExecutor(
	location common.AddressLocation,
	name string,
	arguments []cadence.Value,
	argumentTypes []sema.Type,
	c runtime.Context,
) runtime.Executor {
	return r.Runtime.NewContractFunctionExecutor(location, name, arguments, argumentTypes, r.wrap(c))
}

func (r *observingRuntime) InvokeContractFunction(
	location common.AddressLocation,
	name string,
	arguments []cadence.Value,
	argumentTypes []sema.Type,
	c runtime.Context,
) (cadence.Value, error) {
	return r.Runtime.InvokeContractFunction(location, name, arguments, argumentTypes, r.wrap(c))
}

type observingInterface struct {
	runtime.Interface
	tracer *statementTracer
}

func (i *observingInterface) EmitEvent(event cadence.Event) error {
	i.tracer.onEvent(event)
	return i.Interface.EmitEvent(event)
}

func (i *observingInterface) GenerateUUID() (uint64, error) {
	uuid, err := i.Interface.GenerateUUID()
	if err == nil {
		i.tracer.onUUID(uuid)
	}
	return uuid, err
}

func (i *observingInterface) ResourceOwnerChanged(
	inter *interpreter.Interpreter,
	resource *interpreter.CompositeValue,
	oldOwner common.Address,
	newOwner common.Address,
) {
	i.tracer.onOwnerChange(inter, resource, oldOwner, newOwner)
	i.Interface.ResourceOwnerChanged(inter, resource, oldOwner, newOwner)
}

// statementTracer turns debugger stops into the NDJSON stream.
type statementTracer struct {
	mu        sync.Mutex
	debugger  *interpreter.Debugger
	sourceDir string
	target    *sdk.Transaction
	snapshot  debug.StorageSnapshot

	events []TraceEvent
	steps  int

	// frames is the call stack as last emitted, one entry per interpreter
	// invocation, so a stop at a different depth emits the calls and
	// returns between the two.
	frames []frame

	files      map[string]string // location id -> file written
	sources    map[string]SourceFact
	missing    map[string]bool
	funcIndex  map[string][]funcSpan
	lastFile   string
	lastLine   int
	lastColumn int

	// pendingCreates are UUIDs generated since the last stop whose type is
	// resolved when the value is first observed; resourceTypes remembers
	// every resource type seen by UUID.
	pendingCreates []int
	resourceTypes  map[uint64]string
}

type frame struct {
	name string
}

type funcSpan struct {
	start, end ast.Position
	name       string
}

func newStatementTracer(sourceDir string, target *sdk.Transaction, snapshot debug.StorageSnapshot) *statementTracer {
	return &statementTracer{
		debugger:      interpreter.NewDebugger(),
		sourceDir:     sourceDir,
		target:        target,
		snapshot:      snapshot,
		files:         map[string]string{},
		sources:       map[string]SourceFact{},
		missing:       map[string]bool{},
		funcIndex:     map[string][]funcSpan{},
		resourceTypes: map[uint64]string{},
	}
}

func (t *statementTracer) emit(ev TraceEvent) {
	t.events = append(t.events, ev)
}

func locationID(loc common.Location) string {
	if loc == nil {
		return "unknown"
	}
	return loc.ID()
}

// fileFor writes the source of a location once and returns its file name,
// relative to the source directory so the stream carries no path of the
// machine that recorded it. The
// source of an AddressLocation is the contract code register in the same
// snapshot the execution reads, so it is the code that ran; a transaction's
// source is its script. Anything else is recorded as missing rather than
// guessed.
func (t *statementTracer) fileFor(loc common.Location) string {
	id := locationID(loc)
	if f, ok := t.files[id]; ok {
		return f
	}
	name := sanitize(id) + ".cdc"
	path := filepath.Join(t.sourceDir, name)
	t.files[id] = name

	var code []byte
	origin := ""
	switch l := loc.(type) {
	case common.TransactionLocation:
		code = t.target.Script
		origin = "transaction-script"
	case common.AddressLocation:
		reg := flow.ContractRegisterID(flow.Address(l.Address), l.Name)
		value, err := t.snapshot.Get(reg)
		if err == nil && len(value) > 0 {
			code = value
			origin = "contract-register@prestate"
		}
	}
	if code == nil {
		t.missing[id] = true
		return name
	}
	if err := os.MkdirAll(t.sourceDir, 0o755); err == nil {
		_ = os.WriteFile(path, code, 0o644)
	}
	sum := sha256.Sum256(code)
	t.sources[id] = SourceFact{Location: id, File: name, SHA256: hex.EncodeToString(sum[:]), Origin: origin}
	return name
}

func sanitize(s string) string {
	return strings.Map(func(r rune) rune {
		switch {
		case r >= 'a' && r <= 'z', r >= 'A' && r <= 'Z', r >= '0' && r <= '9', r == '.', r == '-', r == '_':
			return r
		}
		return '_'
	}, s)
}

func (t *statementTracer) sourceFacts() []SourceFact {
	out := make([]SourceFact, 0, len(t.sources))
	for _, s := range t.sources {
		out = append(out, s)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Location < out[j].Location })
	return out
}

func (t *statementTracer) missingSources() []string {
	var out []string
	for id := range t.missing {
		out = append(out, id)
	}
	sort.Strings(out)
	return out
}

// enclosingFunction names the innermost declared function containing pos in
// the program of inter, qualified by its composite (`Vault.withdraw`), or
// `prepare` / `execute` for a transaction's blocks.
func (t *statementTracer) enclosingFunction(inter *interpreter.Interpreter, pos ast.Position) string {
	id := locationID(inter.Location)
	spans, ok := t.funcIndex[id]
	if !ok {
		if inter.Program != nil && inter.Program.Program != nil {
			spans = indexFunctions(inter.Program.Program)
		}
		t.funcIndex[id] = spans
	}
	best := ""
	bestSize := -1
	for _, s := range spans {
		if !within(pos, s.start, s.end) {
			continue
		}
		size := s.end.Offset - s.start.Offset
		if bestSize < 0 || size < bestSize {
			best, bestSize = s.name, size
		}
	}
	if best == "" {
		return "main"
	}
	return best
}

func within(p, start, end ast.Position) bool {
	return p.Offset >= start.Offset && p.Offset <= end.Offset
}

func indexFunctions(program *ast.Program) []funcSpan {
	var spans []funcSpan
	var visitMembers func(prefix string, members *ast.Members)
	addFn := func(prefix string, fn *ast.FunctionDeclaration) {
		if fn == nil {
			return
		}
		spans = append(spans, funcSpan{start: fn.StartPos, end: fn.EndPosition(nil), name: prefix + fn.Identifier.Identifier})
	}
	addSpecial := func(prefix string, special *ast.SpecialFunctionDeclaration) {
		if special == nil || special.FunctionDeclaration == nil {
			return
		}
		name := special.FunctionDeclaration.Identifier.Identifier
		if name == "" {
			name = special.Kind.Keywords()
		}
		spans = append(spans, funcSpan{
			start: special.FunctionDeclaration.StartPos,
			end:   special.FunctionDeclaration.EndPosition(nil),
			name:  prefix + name,
		})
	}
	visitMembers = func(prefix string, members *ast.Members) {
		if members == nil {
			return
		}
		for _, fn := range members.Functions() {
			addFn(prefix, fn)
		}
		for _, special := range members.SpecialFunctions() {
			addSpecial(prefix, special)
		}
		for _, c := range members.Composites() {
			visitMembers(prefix+c.Identifier.Identifier+".", c.Members)
		}
		for _, i := range members.Interfaces() {
			visitMembers(prefix+i.Identifier.Identifier+".", i.Members)
		}
		for _, a := range members.Attachments() {
			visitMembers(prefix+a.Identifier.Identifier+".", a.Members)
		}
	}
	for _, fn := range program.FunctionDeclarations() {
		addFn("", fn)
	}
	for _, c := range program.CompositeDeclarations() {
		visitMembers(c.Identifier.Identifier+".", c.Members)
	}
	for _, i := range program.InterfaceDeclarations() {
		visitMembers(i.Identifier.Identifier+".", i.Members)
	}
	for _, a := range program.AttachmentDeclarations() {
		visitMembers(a.Identifier.Identifier+".", a.Members)
	}
	for _, tx := range program.TransactionDeclarations() {
		addSpecial("", tx.Prepare)
		addSpecial("", tx.Execute)
	}
	return spans
}

func (t *statementTracer) onStop(stop interpreter.Stop) {
	t.mu.Lock()
	defer t.mu.Unlock()

	inter := stop.Interpreter
	defer unmetered(inter)()
	pos := stop.Statement.StartPosition()
	file := t.fileFor(inter.Location)
	name := t.enclosingFunction(inter, pos)

	t.syncFrames(inter, name)

	t.steps++
	t.lastFile, t.lastLine, t.lastColumn = file, pos.Line, pos.Column+1
	t.emit(TraceEvent{Type: "step", File: file, Line: pos.Line, Column: pos.Column + 1})

	activation := t.debugger.CurrentActivation(inter)
	if activation != nil {
		values := activation.ValuesInFunction()
		names := make([]string, 0, len(values))
		for n := range values {
			names = append(names, n)
		}
		sort.Strings(names)
		for _, n := range names {
			val := values[n].GetValue(inter)
			if val == nil {
				continue
			}
			rendered, typ := t.safeRender(inter, val)
			t.emit(TraceEvent{Type: "variable", Name: n, Value: rendered, CadenceType: typ})
		}
	}

	t.observeDestroy(inter, stop.Statement, activation)
}

// syncFrames reconciles the emitted call stack with the interpreter's: the
// interpreter's call stack has one invocation per active call (host functions
// included), so its depth is the depth the current statement runs at, plus
// one for the outermost body, which runs with no invocation of its own.
func (t *statementTracer) syncFrames(inter *interpreter.Interpreter, name string) {
	depth := len(inter.CallStack()) + 1
	for len(t.frames) > depth {
		t.frames = t.frames[:len(t.frames)-1]
		t.emit(TraceEvent{Type: "return"})
	}
	if len(t.frames) == depth && t.frames[depth-1].name != name {
		// Same depth, different function: the previous call returned and a
		// sibling call began between two stops.
		t.frames = t.frames[:depth-1]
		t.emit(TraceEvent{Type: "return"})
	}
	for len(t.frames) < depth-1 {
		t.frames = append(t.frames, frame{name: "<host>"})
		t.emit(TraceEvent{Type: "call", Name: "<host>"})
	}
	if len(t.frames) < depth {
		t.frames = append(t.frames, frame{name: name})
		t.emit(TraceEvent{Type: "call", Name: name})
	}
}

const maxRenderedLen = 512

// unmetered detaches the interpreter's computation and memory gauges until
// the returned function is called. Reading a value through the interpreter --
// a field, a static type, a string form -- is metered like any other access,
// so observing a statement without this changes the transaction's
// computation and therefore its fee: measured on a mainnet mint, 19 units of
// 168. Execution is paused on the debugger channel while a stop is observed,
// so nothing else meters in the meantime.
func unmetered(inter *interpreter.Interpreter) func() {
	config := inter.SharedState.Config
	computation, memory := config.ComputationGauge, config.MemoryGauge
	config.ComputationGauge, config.MemoryGauge = nil, nil
	return func() {
		config.ComputationGauge, config.MemoryGauge = computation, memory
	}
}

// safeRender is render with a floor: a value the interpreter cannot read at
// this statement -- an invalidated container, say -- is shown as unavailable
// rather than taking the replay down with it.
func (t *statementTracer) safeRender(inter *interpreter.Interpreter, val interpreter.Value) (s string, typ string) {
	defer func() {
		if r := recover(); r != nil {
			s, typ = "<unavailable>", ""
		}
	}()
	return t.render(inter, val, 0)
}

// render gives a value's display string and its static type id without
// walking storage-backed containers element by element: a container is
// summarised by count, and a resource by its type and UUID, which is its
// identity.
func (t *statementTracer) render(inter *interpreter.Interpreter, val interpreter.Value, depth int) (string, string) {
	typ := ""
	if st := val.StaticType(inter); st != nil {
		typ = string(st.ID())
	}
	switch v := val.(type) {
	case *interpreter.CompositeValue:
		if v.IsStaleResource(inter) {
			// The source of a `<-` move: the variable still names it, and it
			// holds nothing.
			return fmt.Sprintf("@resource:%s<moved>", v.TypeID()), typ
		}
		if v.Kind == common.CompositeKindResource {
			if uuid := v.ResourceUUID(inter); uuid != nil {
				u := uint64(*uuid)
				t.resourceTypes[u] = string(v.TypeID())
				t.resolvePending(u, string(v.TypeID()))
				return fmt.Sprintf("@resource:%s#%d", v.TypeID(), u), typ
			}
		}
		return fmt.Sprintf("%s(%d fields)", v.TypeID(), v.FieldCount()), typ
	case *interpreter.ArrayValue:
		return fmt.Sprintf("[%d elements]", v.Count()), typ
	case *interpreter.DictionaryValue:
		return fmt.Sprintf("{%d entries}", v.Count()), typ
	case *interpreter.SomeValue:
		if depth < 4 {
			inner := v.InnerValue()
			if inner != nil {
				s, _ := t.render(inter, inner, depth+1)
				return s, typ
			}
		}
	case *interpreter.EphemeralReferenceValue:
		if depth < 4 && v.Value != nil {
			s, _ := t.render(inter, v.Value, depth+1)
			return "&" + s, typ
		}
		return "&" + typ, typ
	case *interpreter.StorageReferenceValue:
		return fmt.Sprintf("&storage(%s %s)", v.TargetStorageAddress.HexWithPrefix(), v.TargetPath), typ
	case interpreter.FunctionValue:
		return "<function>", typ
	}
	s := val.String()
	if len(s) > maxRenderedLen {
		s = s[:maxRenderedLen] + "…"
	}
	return s, typ
}

func (t *statementTracer) onUUID(uuid uint64) {
	t.mu.Lock()
	defer t.mu.Unlock()
	u := uuid
	t.pendingCreates = append(t.pendingCreates, len(t.events))
	// A resource is unowned until it is saved into an account, which Cadence
	// represents as the zero address.
	t.emit(TraceEvent{Type: "resource_create", UUID: &u, Owner: common.ZeroAddress.HexWithPrefix(), File: t.lastFile, Line: t.lastLine})
}

// onEvent records a Cadence event where it was emitted. The value is the
// exported event, so rendering it does not touch the interpreter's gauges.
func (t *statementTracer) onEvent(event cadence.Event) {
	t.mu.Lock()
	defer t.mu.Unlock()
	name := "Unknown"
	if event.EventType != nil {
		name = event.EventType.ID()
	}
	t.emit(TraceEvent{Type: "event", Name: name, Payload: event.String()})
}

// resolvePending fills in the type of a resource_create whose UUID has now
// been observed on a value.
func (t *statementTracer) resolvePending(uuid uint64, typ string) {
	kept := t.pendingCreates[:0]
	for _, idx := range t.pendingCreates {
		ev := &t.events[idx]
		if ev.UUID != nil && *ev.UUID == uuid {
			ev.ResourceType = typ
			continue
		}
		kept = append(kept, idx)
	}
	t.pendingCreates = kept
}

func (t *statementTracer) onOwnerChange(inter *interpreter.Interpreter, resource *interpreter.CompositeValue, oldOwner, newOwner common.Address) {
	t.mu.Lock()
	defer t.mu.Unlock()
	defer unmetered(inter)()
	if resource.IsStaleResource(inter) {
		return
	}
	// A move is recorded against an identity; without a UUID there is
	// nothing to say it is the same resource at both ends.
	u := resource.ResourceUUID(inter)
	if u == nil {
		return
	}
	v := uint64(*u)
	uuid := &v
	t.resourceTypes[v] = string(resource.TypeID())
	t.resolvePending(v, string(resource.TypeID()))
	t.emit(TraceEvent{
		Type:         "resource_move",
		ResourceType: string(resource.TypeID()),
		UUID:         uuid,
		FromOwner:    oldOwner.HexWithPrefix(),
		ToOwner:      newOwner.HexWithPrefix(),
		File:         t.lastFile,
		Line:         t.lastLine,
	})
}

// observeDestroy records a `destroy x` statement against the resource x
// holds when the statement is about to run. Cadence has no destroy hook, so
// this is read at the statement rather than reported by the interpreter, and
// only for a destroyed operand that is a plain identifier.
func (t *statementTracer) observeDestroy(inter *interpreter.Interpreter, stmt ast.Statement, activation *interpreter.VariableActivation) {
	exprStmt, ok := stmt.(*ast.ExpressionStatement)
	if !ok {
		return
	}
	destroy, ok := exprStmt.Expression.(*ast.DestroyExpression)
	if !ok {
		return
	}
	ident, ok := destroy.Expression.(*ast.IdentifierExpression)
	if !ok || activation == nil {
		return
	}
	variable := activation.Find(ident.Identifier.Identifier)
	if variable == nil {
		return
	}
	composite, ok := variable.GetValue(inter).(*interpreter.CompositeValue)
	if !ok || composite.Kind != common.CompositeKindResource || composite.IsStaleResource(inter) {
		return
	}
	ev := TraceEvent{
		Type:         "resource_destroy",
		ResourceType: string(composite.TypeID()),
		Owner:        composite.GetOwner().HexWithPrefix(),
		File:         t.lastFile,
		Line:         t.lastLine,
	}
	u := composite.ResourceUUID(inter)
	if u == nil {
		return
	}
	v := uint64(*u)
	ev.UUID = &v
	t.emit(ev)
}

func (t *statementTracer) finish(execErr error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	for len(t.frames) > 0 {
		t.frames = t.frames[:len(t.frames)-1]
		t.emit(TraceEvent{Type: "return"})
	}
	if execErr != nil {
		t.emit(TraceEvent{Type: "error", Message: execErr.Error()})
	}
	// A UUID whose value was never observed is not known to be a resource
	// of any particular type; it stays a create with no type rather than
	// a guessed one.
	for _, idx := range t.pendingCreates {
		if t.events[idx].ResourceType == "" {
			t.events[idx].ResourceType = "<unobserved>"
		}
	}
}

func (t *statementTracer) writeEvents(w io.Writer) error {
	enc := json.NewEncoder(w)
	enc.SetEscapeHTML(false)
	for _, ev := range t.events {
		if err := enc.Encode(ev); err != nil {
			return err
		}
	}
	return nil
}
