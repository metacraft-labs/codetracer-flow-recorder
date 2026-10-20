package main

// The inputs of one replay, either read from an access node or loaded from a
// transcript a previous run recorded (`--record-inputs` / `--from-inputs`).
//
// A replay reads four things from the network: the transaction's result, its
// block's header, the block's transactions, and the registers of the state
// the block started from. A transcript is exactly those four, so the same FVM
// re-executes the same transaction with no network -- which is what lets a
// build sandbox produce the container from committed inputs instead of a
// committed recording.
//
// A transcript authenticates its own bodies: the header's ID must be the
// block ID it is filed under and every transaction's ID must be recomputable
// from its recorded body, so an edited body is refused rather than replayed.
// Registers cannot be checked that way -- the commitment that covers them is
// a trie this helper does not hold -- so they are what the verdict's
// comparison with the chain's recorded events stands behind.

import (
	"bytes"
	"compress/gzip"
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"sync"
	"time"

	sdk "github.com/onflow/flow-go-sdk"
	sdkgrpc "github.com/onflow/flow-go-sdk/access/grpc"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"

	"github.com/onflow/flow-go/model/flow"
	"github.com/onflow/flow-go/utils/debug"
)

const transcriptSchema = "codetracer-flow-recorder/replay-inputs@1"

// transcriptFile is the name a transcript is written under in its directory.
const transcriptFile = "inputs.json.gz"

// readTranscript reads a transcript directory's document, gzipped or not.
func readTranscript(dir string) ([]byte, error) {
	if raw, err := os.ReadFile(filepath.Join(dir, "inputs.json")); err == nil {
		return raw, nil
	}
	f, err := os.Open(filepath.Join(dir, transcriptFile))
	if err != nil {
		return nil, err
	}
	defer f.Close()
	zr, err := gzip.NewReader(f)
	if err != nil {
		return nil, err
	}
	return io.ReadAll(zr)
}

// EventFact is an event as the chain recorded it, in the form the verdict
// compares: its type and its value's Cadence string.
type EventFact struct {
	Type  string `json:"type"`
	Value string `json:"value"`
}

// OnChainResult is what the chain recorded for the transaction.
type OnChainResult struct {
	Failed          bool        `json:"failed"`
	ErrorMessage    string      `json:"errorMessage,omitempty"`
	ComputationUsed uint64      `json:"computationUsed"`
	Events          []EventFact `json:"events"`
}

// RegisterRead is one register of the parent state, as the access node
// answered it. Owner and key are hex because a key may be binary (a slab
// index); Present is false for a register the node reported as absent.
type RegisterRead struct {
	Owner   string `json:"owner"`
	Key     string `json:"key"`
	Present bool   `json:"present"`
	Value   []byte `json:"value,omitempty"`
}

// Transcript is the --record-inputs document.
type Transcript struct {
	Schema        string             `json:"schema"`
	AccessNode    string             `json:"accessNode"`
	RecordedAt    string             `json:"recordedAt"`
	TransactionID string             `json:"transactionId"`
	BlockID       string             `json:"blockId"`
	Header        *flow.Header       `json:"header"`
	Transactions  []*sdk.Transaction `json:"transactions"`
	OnChain       OnChainResult      `json:"onChain"`
	Registers     []RegisterRead     `json:"registers"`
}

type acquired struct {
	header       *flow.Header
	transactions []*sdk.Transaction
	onChain      OnChainResult
	prestate     debug.StorageSnapshot
	recorder     *recordingSnapshot
	accessNode   string
	close        func()
}

func onChainFromResult(r *sdk.TransactionResult) OnChainResult {
	out := OnChainResult{
		Failed:          r.Error != nil,
		ComputationUsed: r.ComputationUsage,
		Events:          []EventFact{},
	}
	if r.Error != nil {
		out.ErrorMessage = r.Error.Error()
	}
	for _, ev := range r.Events {
		out.Events = append(out.Events, EventFact{Type: ev.Type, Value: ev.Value.String()})
	}
	return out
}

// acquire reads the replay's inputs from the access node, or from the
// transcript in fromInputs when it is set.
func acquire(ctx context.Context, txID flow.Identifier, accessNode string, chain flow.Chain, fromInputs string) (*acquired, error) {
	if fromInputs != "" {
		return loadTranscript(fromInputs, txID)
	}

	flowClient, err := sdkgrpc.NewClient(
		accessNode,
		sdkgrpc.WithGRPCDialOptions(
			grpc.WithTransportCredentials(insecure.NewCredentials()),
			grpc.WithDefaultCallOptions(grpc.MaxCallRecvMsgSize(64<<20)),
		),
	)
	if err != nil {
		return nil, fmt.Errorf("access node client: %w", err)
	}
	txResult, err := flowClient.GetTransactionResult(ctx, sdk.Identifier(txID))
	if err != nil {
		flowClient.Close()
		return nil, fmt.Errorf("fetching transaction result: %w", err)
	}
	blockID := flow.Identifier(txResult.BlockID)
	header, err := debug.GetAccessAPIBlockHeader(ctx, flowClient.RPCClient(), blockID)
	if err != nil {
		flowClient.Close()
		return nil, fmt.Errorf("fetching block header: %w", err)
	}
	blockTxs, err := flowClient.GetTransactionsByBlockID(ctx, sdk.Identifier(blockID))
	if err != nil {
		flowClient.Close()
		return nil, fmt.Errorf("fetching block transactions: %w", err)
	}
	remoteClient, err := debug.NewExecutionDataRemoteClient(accessNode, chain)
	if err != nil {
		flowClient.Close()
		return nil, fmt.Errorf("execution data client: %w", err)
	}
	// StorageSnapshot(h) reads the registers RESULTING from h-1, i.e. the
	// state the block at h started from.
	remote, err := remoteClient.StorageSnapshot(header.Height, header.ID())
	if err != nil {
		flowClient.Close()
		remoteClient.Close()
		return nil, fmt.Errorf("storage snapshot at %d: %w", header.Height-1, err)
	}
	rec := &recordingSnapshot{backing: remote, reads: map[flow.RegisterID]RegisterRead{}}
	return &acquired{
		header:       header,
		transactions: blockTxs,
		onChain:      onChainFromResult(txResult),
		prestate:     rec,
		recorder:     rec,
		accessNode:   accessNode,
		close: func() {
			remoteClient.Close()
			flowClient.Close()
		},
	}, nil
}

// save writes the transcript of an acquisition from the network.
func (a *acquired) save(dir string, txID flow.Identifier) error {
	if a.recorder == nil {
		return fmt.Errorf("--record-inputs needs the inputs to come from an access node, not a transcript")
	}
	t := Transcript{
		Schema:        transcriptSchema,
		AccessNode:    a.accessNode,
		RecordedAt:    time.Now().UTC().Format(time.RFC3339),
		TransactionID: txID.String(),
		BlockID:       a.header.ID().String(),
		Header:        a.header,
		Transactions:  a.transactions,
		OnChain:       a.onChain,
		Registers:     a.recorder.sorted(),
	}
	data, err := json.MarshalIndent(t, "", " ")
	if err != nil {
		return err
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	// Gzipped: a transcript is mostly contract code, base64-encoded, and a
	// block's second transaction carries the first's reads as well -- 1.24 MB
	// as JSON on mainnet, over the 1 MB a committed file may be.
	var buf bytes.Buffer
	zw, err := gzip.NewWriterLevel(&buf, gzip.BestCompression)
	if err != nil {
		return err
	}
	zw.ModTime = time.Time{}
	if _, err := zw.Write(append(data, '\n')); err != nil {
		return err
	}
	if err := zw.Close(); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(dir, transcriptFile), buf.Bytes(), 0o644)
}

func loadTranscript(dir string, txID flow.Identifier) (*acquired, error) {
	data, err := readTranscript(dir)
	if err != nil {
		return nil, fmt.Errorf("reading transcript: %w", err)
	}
	var t Transcript
	if err := json.Unmarshal(data, &t); err != nil {
		return nil, fmt.Errorf("parsing transcript: %w", err)
	}
	if t.Schema != transcriptSchema {
		return nil, fmt.Errorf("transcript schema is %q, this helper reads %q", t.Schema, transcriptSchema)
	}
	if t.TransactionID != txID.String() {
		return nil, fmt.Errorf("transcript is for transaction %s, not %s", t.TransactionID, txID)
	}
	if t.Header == nil || t.Header.ID().String() != t.BlockID {
		return nil, fmt.Errorf("transcript header does not hash to its block id %s", t.BlockID)
	}
	// The ID is recomputed from the recorded body, so a body edited after
	// recording no longer matches the transaction it claims to be.
	found := false
	for _, tx := range t.Transactions {
		if flow.Identifier(tx.ID()) == txID {
			found = true
		}
	}
	if !found {
		return nil, fmt.Errorf("transcript's block transactions do not include %s", txID)
	}
	snap := &transcriptSnapshot{values: map[flow.RegisterID]RegisterRead{}}
	for _, r := range t.Registers {
		owner, err := hex.DecodeString(r.Owner)
		if err != nil {
			return nil, fmt.Errorf("transcript register owner %q: %w", r.Owner, err)
		}
		key, err := hex.DecodeString(r.Key)
		if err != nil {
			return nil, fmt.Errorf("transcript register key %q: %w", r.Key, err)
		}
		snap.values[flow.NewRegisterID(flow.BytesToAddress(owner), string(key))] = r
	}
	return &acquired{
		header:       t.Header,
		transactions: t.Transactions,
		onChain:      t.OnChain,
		prestate:     snap,
		accessNode:   t.AccessNode,
		close:        func() {},
	}, nil
}

// recordingSnapshot forwards to the access node and keeps the first answer
// for every register, which is the parent state's value: it sits beneath the
// caching snapshot the preceding transactions write into, so a register they
// changed is never read from here again.
type recordingSnapshot struct {
	mu      sync.Mutex
	backing debug.StorageSnapshot
	reads   map[flow.RegisterID]RegisterRead
}

func (s *recordingSnapshot) Get(id flow.RegisterID) (flow.RegisterValue, error) {
	value, err := s.backing.Get(id)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	if _, seen := s.reads[id]; !seen {
		s.reads[id] = RegisterRead{
			Owner:   hex.EncodeToString([]byte(id.Owner)),
			Key:     hex.EncodeToString([]byte(id.Key)),
			Present: value != nil,
			Value:   value,
		}
	}
	s.mu.Unlock()
	return value, nil
}

func (s *recordingSnapshot) sorted() []RegisterRead {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]RegisterRead, 0, len(s.reads))
	for _, r := range s.reads {
		out = append(out, r)
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Owner != out[j].Owner {
			return out[i].Owner < out[j].Owner
		}
		return out[i].Key < out[j].Key
	})
	return out
}

// transcriptSnapshot answers from a transcript and refuses a register it
// does not hold: answering "absent" would let a replay diverge silently on a
// read the recording run never made.
type transcriptSnapshot struct {
	values map[flow.RegisterID]RegisterRead
}

func (s *transcriptSnapshot) Get(id flow.RegisterID) (flow.RegisterValue, error) {
	r, ok := s.values[id]
	if !ok {
		return nil, fmt.Errorf("register %x/%x is not in the transcript", []byte(id.Owner), []byte(id.Key))
	}
	if !r.Present {
		return nil, nil
	}
	return r.Value, nil
}
