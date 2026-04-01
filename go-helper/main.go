// Package main implements a helper program that executes Cadence source files
// using the real Cadence runtime/interpreter and emits NDJSON trace events
// on stdout for consumption by the Rust recorder.
//
// Usage:
//
//	cadence-trace-helper <source-file.cdc>
//
// Output format (one JSON object per line on stdout):
//
//	{"type":"step","file":"path.cdc","line":3}
//	{"type":"variable","name":"a","value":"10","cadence_type":"Int"}
//	{"type":"call","name":"compute"}
//	{"type":"return","value":"94"}
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/onflow/cadence"
	"github.com/onflow/cadence/common"
	"github.com/onflow/cadence/interpreter"
	jsoncdc "github.com/onflow/cadence/encoding/json"
)

// TraceEvent represents a single NDJSON trace line.
type TraceEvent struct {
	Type         string `json:"type"`
	File         string `json:"file,omitempty"`
	Line         int    `json:"line,omitempty"`
	Name         string `json:"name,omitempty"`
	Value        string `json:"value,omitempty"`
	CadenceType  string `json:"cadence_type,omitempty"`
	ResourceType string `json:"resource_type,omitempty"`
	UUID         uint64 `json:"uuid,omitempty"`
	Owner        string `json:"owner,omitempty"`
	FromOwner    string `json:"from_owner,omitempty"`
	ToOwner      string `json:"to_owner,omitempty"`
}

// TracerConfig holds configuration for resource tracking.
type TracerConfig struct {
	// ResourceOwnerChangeHandlerEnabled enables resource lifecycle event emission.
	ResourceOwnerChangeHandlerEnabled bool
}

var (
	encoder    *json.Encoder
	sourceFile string
	tracerConfig = TracerConfig{
		ResourceOwnerChangeHandlerEnabled: true,
	}
)

func emit(event TraceEvent) {
	_ = encoder.Encode(event)
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintf(os.Stderr, "usage: cadence-trace-helper <source-file.cdc>\n")
		fmt.Fprintf(os.Stderr, "       cadence-trace-helper replay --tx-hash <hash> --access-node <url> [--source-dir <dir>]\n")
		os.Exit(1)
	}

	encoder = json.NewEncoder(os.Stdout)
	encoder.SetEscapeHTML(false)

	// Check if we are in replay mode.
	if os.Args[1] == "replay" {
		runReplayMode(os.Args[2:])
		return
	}

	sourceFile = os.Args[1]
	absPath, err := filepath.Abs(sourceFile)
	if err == nil {
		sourceFile = absPath
	}

	sourceBytes, err := os.ReadFile(sourceFile)
	if err != nil {
		fmt.Fprintf(os.Stderr, "error reading source file: %v\n", err)
		os.Exit(1)
	}

	source := string(sourceBytes)

	// Execute the Cadence script using the interpreter directly.
	// The Cadence v1.x API uses an interpreter-based approach.
	err = executeAndTrace(source)
	if err != nil {
		fmt.Fprintf(os.Stderr, "cadence execution error: %v\n", err)
		os.Exit(1)
	}
}

// emitResourceCreate emits a resource_create NDJSON event.
func emitResourceCreate(resourceType string, uuid uint64, owner string, file string, line int) {
	if !tracerConfig.ResourceOwnerChangeHandlerEnabled {
		return
	}
	emit(TraceEvent{
		Type:         "resource_create",
		ResourceType: resourceType,
		UUID:         uuid,
		Owner:        owner,
		File:         file,
		Line:         line,
	})
}

// emitResourceMove emits a resource_move NDJSON event when ownership changes.
func emitResourceMove(resourceType string, uuid uint64, fromOwner string, toOwner string, file string, line int) {
	if !tracerConfig.ResourceOwnerChangeHandlerEnabled {
		return
	}
	emit(TraceEvent{
		Type:         "resource_move",
		ResourceType: resourceType,
		UUID:         uuid,
		FromOwner:    fromOwner,
		ToOwner:      toOwner,
		File:         file,
		Line:         line,
	})
}

// emitResourceDestroy emits a resource_destroy NDJSON event.
func emitResourceDestroy(resourceType string, uuid uint64, owner string, file string, line int) {
	if !tracerConfig.ResourceOwnerChangeHandlerEnabled {
		return
	}
	emit(TraceEvent{
		Type:         "resource_destroy",
		ResourceType: resourceType,
		UUID:         uuid,
		Owner:        owner,
		File:         file,
		Line:         line,
	})
}

// runReplayMode handles the "replay" subcommand.
// It parses --tx-hash, --access-node, and optional --source-dir flags,
// fetches the transaction from the Flow Access API, and executes its
// Cadence script through the emulator with tracing hooks.
func runReplayMode(args []string) {
	var txHash, accessNode, sourceDir string

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--tx-hash":
			if i+1 < len(args) {
				i++
				txHash = args[i]
			}
		case "--access-node":
			if i+1 < len(args) {
				i++
				accessNode = args[i]
			}
		case "--source-dir":
			if i+1 < len(args) {
				i++
				sourceDir = args[i]
			}
		}
	}

	if txHash == "" {
		fmt.Fprintf(os.Stderr, "replay: --tx-hash is required\n")
		os.Exit(1)
	}
	if accessNode == "" {
		accessNode = "access.mainnet.nodes.onflow.org:9000"
	}

	err := replayTransaction(txHash, accessNode, sourceDir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "replay error: %v\n", err)
		os.Exit(1)
	}
}

// replayTransaction fetches a transaction from the Flow Access API and
// replays its Cadence script with tracing enabled.
//
// TODO(M5): This is a stub implementation. The full version will:
//   - Use flow-go-sdk to connect to the Access Node via gRPC
//   - Fetch the transaction by hash using client.GetTransaction()
//   - Extract the Cadence script and arguments
//   - Set up the Flow emulator in fork mode at the transaction's block height
//   - Execute the script through the emulator with tracing hooks
//
// For now, it emits an error indicating that the transaction fetch is not yet
// connected to the real Access API.
func replayTransaction(txHash string, accessNode string, sourceDir string) error {
	_ = sourceDir // Will be used for source mapping in the full implementation.

	// Set the sourceFile for trace events.
	sourceFile = fmt.Sprintf("tx_%s.cdc", txHash)

	fmt.Fprintf(os.Stderr, "replay: fetching transaction %s from %s\n", txHash, accessNode)

	// TODO(M5): Replace this stub with real Access API calls:
	//
	//   import "github.com/onflow/flow-go-sdk/access/grpc"
	//
	//   client, err := grpc.NewClient(accessNode)
	//   tx, err := client.GetTransaction(ctx, flow.HexToID(txHash))
	//   script := string(tx.Script)
	//   args := tx.Arguments
	//
	// Then execute the script through the emulator in fork mode and trace it.

	return fmt.Errorf(
		"replay mode is not yet fully implemented: "+
			"transaction fetch from Access API requires flow-go-sdk integration (tx=%s, node=%s)",
		txHash, accessNode,
	)
}

func executeAndTrace(source string) error {
	// For Cadence v1.x, we use the interpreter package directly.
	// The runtime.ExecuteScript API requires a full runtime interface;
	// for tracing purposes, we set up a minimal checker+interpreter pipeline.

	// Parse and check the program.
	location := common.ScriptLocation{0x1}

	// Use the Cadence standard library checker and interpreter.
	// This is a simplified approach; a production version would set up
	// the full runtime pipeline with OnStatement/OnFunctionInvocation hooks.
	program, err := cadence.NewProgram(source, location)
	if err != nil {
		return fmt.Errorf("failed to parse/check program: %w", err)
	}

	// Set up interpreter with tracing hooks.
	inter, err := program.NewInterpreter(
		interpreter.WithOnStatementHandler(func(
			inter *interpreter.Interpreter,
			statement interpreter.Statement,
			location common.Location,
		) {
			pos := statement.StartPosition()
			emit(TraceEvent{
				Type: "step",
				File: sourceFile,
				Line: pos.Line,
			})
		}),
		interpreter.WithOnFunctionInvocationHandler(func(
			inter *interpreter.Interpreter,
			functionType *interpreter.FunctionStaticType,
		) {
			emit(TraceEvent{
				Type: "call",
				Name: "function",
			})
		}),
	)
	if err != nil {
		return fmt.Errorf("failed to create interpreter: %w", err)
	}

	// Execute.
	value, err := inter.Invoke("main")
	if err != nil {
		return fmt.Errorf("failed to invoke main: %w", err)
	}

	// Emit the final return value.
	if value != nil {
		cadenceValue := interpreter.ExportValue(value, inter, interpreter.EmptyLocationRange)
		jsonBytes, _ := jsoncdc.Encode(cadenceValue)
		emit(TraceEvent{
			Type:        "return",
			Value:       fmt.Sprintf("%v", value),
			CadenceType: string(jsonBytes),
		})
	}

	return nil
}
