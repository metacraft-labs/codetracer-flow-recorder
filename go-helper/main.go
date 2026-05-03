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
//	{"type":"call","name":"compute","args":[{"name":"x","value":"10","cadence_type":"Int"}]}
//	{"type":"return","value":"94"}
//	{"type":"event","name":"MyEvent","payload":"MyEvent(message: \"done\")"}
//	{"type":"error","message":"cadence execution error: ..."}
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/onflow/cadence"
	"github.com/onflow/cadence/common"
	"github.com/onflow/cadence/interpreter"
	"github.com/onflow/cadence/runtime"
	runtime_utils "github.com/onflow/cadence/test_utils/runtime_utils"
)

// TraceEvent represents a single NDJSON trace line.
type TraceEvent struct {
	Type        string     `json:"type"`
	File        string     `json:"file,omitempty"`
	Line        int        `json:"line,omitempty"`
	Name        string     `json:"name,omitempty"`
	Value       string     `json:"value,omitempty"`
	Payload     string     `json:"payload,omitempty"`
	CadenceType string     `json:"cadence_type,omitempty"`
	Message     string     `json:"message,omitempty"`
	Args        []TraceArg `json:"args,omitempty"`
}

// TraceArg is the helper-side schema for a staged CodeTracer call argument.
type TraceArg struct {
	Name        string `json:"name"`
	Value       string `json:"value"`
	CadenceType string `json:"cadence_type,omitempty"`
}

var (
	encoder    *json.Encoder
	sourceFile string
)

func emit(event TraceEvent) {
	_ = encoder.Encode(event)
}

type functionInfo struct {
	lineToName   map[int]string
	paramsByName map[string][]string
}

// parseFunctionSignature extracts the function name and formal parameter names
// from a single-line Cadence declaration such as `pub fun add(x: Int, y: Int)`.
// It intentionally ignores parameter type syntax after `:` because values and
// resolved Cadence types are recovered from the live interpreter activation.
func parseFunctionSignature(trimmed string) (string, []string) {
	idx := strings.Index(trimmed, "fun ")
	if idx < 0 {
		return "", nil
	}

	rest := trimmed[idx+4:]
	openIdx := strings.Index(rest, "(")
	if openIdx < 0 {
		return "", nil
	}

	name := strings.TrimSpace(rest[:openIdx])
	if name == "" {
		return "", nil
	}

	closeIdx := strings.Index(rest[openIdx+1:], ")")
	if closeIdx < 0 {
		return name, nil
	}

	paramsText := rest[openIdx+1 : openIdx+1+closeIdx]
	if strings.TrimSpace(paramsText) == "" {
		return name, nil
	}

	var params []string
	for _, part := range strings.Split(paramsText, ",") {
		beforeType := strings.SplitN(part, ":", 2)[0]
		paramName := strings.TrimSpace(beforeType)
		if paramName != "" {
			params = append(params, paramName)
		}
	}

	return name, params
}

// buildFunctionInfo parses the source to create a mapping from line number
// to the enclosing function name and a function-name to formal-parameter map.
// Lines not inside any function map to "main".
func buildFunctionInfo(source []byte) functionInfo {
	lines := strings.Split(string(source), "\n")
	result := functionInfo{
		lineToName:   make(map[int]string),
		paramsByName: make(map[string][]string),
	}

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
		name, params := parseFunctionSignature(trimmed)
		if name != "" {
			if _, exists := result.paramsByName[name]; !exists {
				result.paramsByName[name] = params
			}
			// Push function with the current brace depth (before counting this line)
			stack = append(stack, funcRange{
				name:       name,
				startLine:  lineNo,
				braceDepth: braceDepth,
			})
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
			result.lineToName[lineNo] = stack[len(stack)-1].name
		} else {
			result.lineToName[lineNo] = "main"
		}
	}

	return result
}

func traceArgFromVariable(name string, variable interpreter.Variable, interp *interpreter.Interpreter) TraceArg {
	val := variable.GetValue(interp)
	valueStr := fmt.Sprintf("%v", val)
	typeStr := ""
	if val != nil {
		cadenceVal, err := runtime.ExportValue(
			val,
			interp,
			interpreter.EmptyLocationRange,
		)
		if err == nil && cadenceVal != nil {
			typeStr = cadenceVal.Type().ID()
		}
	}
	return TraceArg{
		Name:        name,
		Value:       valueStr,
		CadenceType: typeStr,
	}
}

func traceArgsFromActivation(
	activation *interpreter.VariableActivation,
	paramNames []string,
	interp *interpreter.Interpreter,
) []TraceArg {
	if activation == nil || len(paramNames) == 0 {
		return nil
	}

	values := activation.FunctionValues()
	args := make([]TraceArg, 0, len(paramNames))
	for _, name := range paramNames {
		variable, ok := values[name]
		if !ok {
			continue
		}
		args = append(args, traceArgFromVariable(name, variable, interp))
	}
	return args
}

func traceEventName(event cadence.Event) string {
	if event.EventType == nil {
		return "Unknown"
	}
	if event.EventType.QualifiedIdentifier != "" {
		return event.EventType.QualifiedIdentifier
	}
	return event.EventType.ID()
}

func traceEventPayload(event cadence.Event) string {
	if event.EventType == nil {
		return fmt.Sprintf("%v", event)
	}
	return event.String()
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
		emit(TraceEvent{
			Type:    "error",
			Message: fmt.Sprintf("%v", err),
		})
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
		OnEmitEvent: func(event cadence.Event) error {
			emit(TraceEvent{
				Type:    "event",
				Name:    traceEventName(event),
				Payload: traceEventPayload(event),
			})
			return nil
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
		defer func() {
			if recovered := recover(); recovered != nil {
				resultCh <- execResultT{err: fmt.Errorf("panic: %v", recovered)}
			}
		}()
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

	// Pre-parse source to build a line→function map and formal-parameter map.
	funcInfo := buildFunctionInfo(source)

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
		currentFunc := funcInfo.lineToName[pos.Line]
		if currentFunc == "" {
			currentFunc = "main"
		}
		currentArgs := traceArgsFromActivation(
			activation,
			funcInfo.paramsByName[currentFunc],
			stop.Interpreter,
		)

		// Detect function transitions by comparing the current function
		// name (derived from source line) with the previous one.
		if currentFunc != prevFuncName {
			if prevFuncName == "" {
				// First statement — emit the initial function call.
				emit(TraceEvent{
					Type: "call",
					Name: currentFunc,
					Args: currentArgs,
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
						Args: currentArgs,
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
