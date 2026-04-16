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
	"strings"

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

// buildFunctionMap parses the source to create a mapping from line number
// to the enclosing function name. This handles Cadence's "fun <name>"
// declarations. Lines not inside any function map to "main".
func buildFunctionMap(source []byte) map[int]string {
	lines := strings.Split(string(source), "\n")
	result := make(map[int]string)

	type funcRange struct {
		name       string
		startLine  int
		braceDepth int
	}

	var stack []funcRange
	braceDepth := 0

	for lineNum, line := range lines {
		lineNo := lineNum + 1 // 1-based
		trimmed := strings.TrimSpace(line)

		// Check for function declaration
		if idx := strings.Index(trimmed, "fun "); idx >= 0 {
			rest := trimmed[idx+4:]
			name := ""
			for _, ch := range rest {
				if ch == '(' || ch == ':' || ch == ' ' || ch == '{' {
					break
				}
				name += string(ch)
			}
			if name != "" {
				// Push function with the current brace depth (before counting this line)
				stack = append(stack, funcRange{
					name:       name,
					startLine:  lineNo,
					braceDepth: braceDepth,
				})
			}
		}

		// Count braces on this line
		for _, ch := range line {
			if ch == '{' {
				braceDepth++
			} else if ch == '}' {
				braceDepth--
				// Check if we're closing a function
				if len(stack) > 0 && braceDepth == stack[len(stack)-1].braceDepth {
					stack = stack[:len(stack)-1]
				}
			}
		}

		// Assign the current function name to this line
		if len(stack) > 0 {
			result[lineNo] = stack[len(stack)-1].name
		} else {
			result[lineNo] = "main"
		}
	}

	return result
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

	// Track which function we're in by source line analysis.
	// The Cadence debugger's activation depth doesn't change between
	// function calls in all cases, so we detect function transitions
	// by watching for the source line to jump into a different function's
	// body (i.e., the line is within a different "fun" declaration).
	prevFuncName := ""
	var callStack []string

	// Pre-parse source to build a line→function map.
	funcMap := buildFunctionMap(source)

	// Read the first stop from the Stops() channel.
	stops := debugger.Stops()
	for {
		var stop interpreter.Stop
		select {
		case stop = <-stops:
			// Got a stop from the debugger.
		case result := <-resultCh:
			// Execution finished — emit returns for remaining stack frames.
			for len(callStack) > 0 {
				emit(TraceEvent{Type: "return"})
				callStack = callStack[:len(callStack)-1]
			}
			if result.value != nil {
				emit(TraceEvent{
					Type:  "return",
					Value: fmt.Sprintf("%v", result.value),
				})
			}
			return result.err
		}

		activation := debugger.CurrentActivation(stop.Interpreter)

		stmt := stop.Statement
		pos := stmt.StartPosition()
		currentFunc := funcMap[pos.Line]
		if currentFunc == "" {
			currentFunc = "main"
		}

		// Detect function transitions by comparing the current function
		// name (derived from source line) with the previous one.
		if currentFunc != prevFuncName {
			if prevFuncName == "" {
				// First statement — emit the initial function call.
				emit(TraceEvent{
					Type: "call",
					Name: currentFunc,
				})
				callStack = append(callStack, currentFunc)
			} else {
				// Check if we're returning to a function already on the stack.
				returning := false
				for i := len(callStack) - 2; i >= 0; i-- {
					if callStack[i] == currentFunc {
						// Pop the call stack back to this function.
						for len(callStack) > 0 && callStack[len(callStack)-1] != currentFunc {
							emit(TraceEvent{Type: "return"})
							callStack = callStack[:len(callStack)-1]
						}
						returning = true
						break
					}
				}
				if !returning {
					// Entering a new function — emit call.
					emit(TraceEvent{
						Type: "call",
						Name: currentFunc,
					})
					callStack = append(callStack, currentFunc)
				}
			}
			prevFuncName = currentFunc
		}

		emit(TraceEvent{
			Type: "step",
			File: sourceFile,
			Line: pos.Line,
		})

		// Extract local variables from the current activation.
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
