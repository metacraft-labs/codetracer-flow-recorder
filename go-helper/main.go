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

	"github.com/onflow/cadence/common"
	"github.com/onflow/cadence/interpreter"
	"github.com/onflow/cadence/runtime"
	runtime_utils "github.com/onflow/cadence/test_utils/runtime_utils"
)

// TraceEvent represents a single NDJSON trace line.
type TraceEvent struct {
	Type        string `json:"type"`
	File        string `json:"file,omitempty"`
	Line        int    `json:"line,omitempty"`
	Name        string `json:"name,omitempty"`
	Value       string `json:"value,omitempty"`
	CadenceType string `json:"cadence_type,omitempty"`
}

var (
	encoder    *json.Encoder
	sourceFile string
)

func emit(event TraceEvent) {
	_ = encoder.Encode(event)
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintf(os.Stderr, "usage: cadence-trace-helper <source-file.cdc>\n")
		os.Exit(1)
	}

	encoder = json.NewEncoder(os.Stdout)
	encoder.SetEscapeHTML(false)

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

	source := sourceBytes

	err = executeAndTrace(source)
	if err != nil {
		fmt.Fprintf(os.Stderr, "cadence execution error: %v\n", err)
		os.Exit(1)
	}
}

func executeAndTrace(source []byte) error {
	// Create a Debugger that will pause at every statement.
	debugger := interpreter.NewDebugger()

	// Set up the runtime with the debugger attached.
	config := runtime.Config{
		Debugger: debugger,
	}
	rt := runtime.NewInterpreterRuntime(config)

	// Use the test runtime interface which provides a minimal environment
	// suitable for script execution (no on-chain state needed).
	location := common.ScriptLocation{0x1}
	runtimeInterface := &runtime_utils.TestRuntimeInterface{
		Storage: runtime_utils.NewTestLedger(nil, nil),
		OnGetCode: func(loc runtime.Location) ([]byte, error) {
			return source, nil
		},
	}

	ctx := runtime.Context{
		Interface: runtimeInterface,
		Location:  location,
	}

	// Request an immediate pause so the debugger stops at the first statement.
	debugger.RequestPause()

	// Run the script in a goroutine since ExecuteScript blocks until completion.
	// When execution finishes, the goroutine closes the done channel.
	type execResultT struct {
		value interface{}
		err   error
	}
	resultCh := make(chan execResultT, 1)

	go func() {
		val, err := rt.ExecuteScript(runtime.Script{Source: source}, ctx)
		resultCh <- execResultT{val, err}
	}()

	// Read the first stop from the Stops() channel.
	// The Debugger.onStatement method sends a Stop to the stops channel
	// whenever the runtime hits a statement and a pause was requested.
	// After processing, we call Continue() then RequestPause() to step
	// through each statement.
	stops := debugger.Stops()
	for {
		var stop interpreter.Stop
		select {
		case stop = <-stops:
			// Got a stop from the debugger.
		case result := <-resultCh:
			// Execution finished — no more stops to process.
			if result.value != nil {
				emit(TraceEvent{
					Type:  "return",
					Value: fmt.Sprintf("%v", result.value),
				})
			}
			return result.err
		}

		stmt := stop.Statement
		pos := stmt.StartPosition()

		emit(TraceEvent{
			Type: "step",
			File: sourceFile,
			Line: pos.Line,
		})

		// Extract local variables from the current activation.
		activation := debugger.CurrentActivation(stop.Interpreter)
		if activation != nil {
			for name, variable := range activation.FunctionValues() {
				val := variable.GetValue(stop.Interpreter)
				if val != nil {
					cadenceVal, err := runtime.ExportValue(
						val,
						stop.Interpreter,
						interpreter.EmptyLocationRange,
					)
					valueStr := fmt.Sprintf("%v", val)
					typeStr := ""
					if err == nil && cadenceVal != nil {
						typeStr = cadenceVal.Type().ID()
					}
					emit(TraceEvent{
						Type:        "variable",
						Name:        name,
						Value:       valueStr,
						CadenceType: typeStr,
					})
				}
			}
		}

		// Request another pause and continue execution so we stop at
		// the next statement.
		debugger.RequestPause()
		debugger.Continue()
	}
}
