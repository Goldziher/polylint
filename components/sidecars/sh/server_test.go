package main

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"strings"
	"testing"

	"github.com/vmihailenco/msgpack/v5"
)

func wireInt(n int64) map[string]any   { return map[string]any{"Integer": n} }
func wireBool(b bool) map[string]any   { return map[string]any{"Boolean": b} }
func wireTab(kv ...any) map[string]any { return map[string]any{"Table": pairs(kv...)} }

func pairs(kv ...any) []any {
	out := make([]any, 0, len(kv)/2)
	for i := 0; i < len(kv); i += 2 {
		out = append(out, []any{kv[i], kv[i+1]})
	}
	return out
}

func mustPack(t *testing.T, v any) []byte {
	t.Helper()
	out, err := msgpack.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return out
}

// response is the decoded shape of a Response.
type response struct {
	Result map[string]msgpack.RawMessage `msgpack:"result"`
	Logs   []wireLog                     `msgpack:"logs"`
}

func call(t *testing.T, s *server, request map[string]any, body []byte) response {
	t.Helper()
	out, err := s.call(mustPack(t, request), body)
	if err != nil {
		t.Fatalf("call: %v", err)
	}
	var resp response
	if err := msgpack.Unmarshal(out, &resp); err != nil {
		t.Fatalf("decode response: %v", err)
	}
	return resp
}

func register(t *testing.T, s *server, id uint32, indent int64, tabs bool) {
	t.Helper()
	options := wireTab()
	if tabs {
		options = wireTab("use_tabs", wireBool(true))
	}
	resp := call(t, s, map[string]any{"RegisterConfig": []any{id, wireTab("indent_width", wireInt(indent), "options", options)}}, nil)
	if _, ok := resp.Result["Ok"]; !ok {
		t.Fatalf("RegisterConfig failed: %v", resp.Result)
	}
}

// formatRequestFor is a v3 Format header: `{"Format": [engine, config, [path, language, len]]}`.
func formatRequestFor(id uint32, path string, n int) map[string]any {
	return map[string]any{"Format": []any{
		"shfmt", id, []any{path, "shell", uint64(n)}, //nolint:gosec // n is a small test length
	}}
}

func TestInit(t *testing.T) {
	tests := []struct {
		name     string
		protocol uint32
		wantOK   bool
	}{
		{"current protocol", protocolVersion, true},
		{"older protocol", protocolVersion - 1, false},
		{"newer protocol", protocolVersion + 1, false},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := newServer("1.2.3")
			resp := call(t, s, map[string]any{"Init": []any{tt.protocol, "x", uint32(1), uint64(1), "Debug"}}, nil)
			_, ok := resp.Result["Ok"]
			if ok != tt.wantOK {
				t.Fatalf("ok = %v, want %v (%v)", ok, tt.wantOK, resp.Result)
			}
		})
	}
}

func TestFormat(t *testing.T) {
	const unformatted = "greet() {\n        echo hi\n}\n"
	tests := []struct {
		name     string
		indent   int64
		tabs     bool
		src      string
		want     string
		wantKind string
	}{
		{"two spaces", 2, false, unformatted, "greet() {\n  echo hi\n}\n", "formatted"},
		{"four spaces", 4, false, unformatted, "greet() {\n    echo hi\n}\n", "formatted"},
		{"tabs", 2, true, unformatted, "greet() {\n\techo hi\n}\n", "formatted"},
		{"already formatted", 2, false, "echo hi\n", "", "unchanged"},
		{"syntax error is left alone", 2, false, "if [ 1 ]; then\n", "", "unchanged"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := newServer("1.2.3")
			register(t, s, 1, tt.indent, tt.tabs)
			resp := call(t, s, formatRequestFor(1, "run.sh", len(tt.src)), []byte(tt.src))
			var formatted msgpack.RawMessage
			var ok bool
			if formatted, ok = resp.Result["Ok"]; !ok {
				t.Fatalf("not ok: %v", resp.Result)
			}
			var payload map[string]msgpack.RawMessage
			if err := msgpack.Unmarshal(formatted, &payload); err != nil {
				t.Fatal(err)
			}
			raw := payload["Formatted"]
			var unchanged string
			if err := msgpack.Unmarshal(raw, &unchanged); err == nil {
				if tt.wantKind != "unchanged" || unchanged != "Unchanged" {
					t.Fatalf("got %q, want kind %s", unchanged, tt.wantKind)
				}
				return
			}
			var text map[string]string
			if err := msgpack.Unmarshal(raw, &text); err != nil {
				t.Fatal(err)
			}
			if tt.wantKind != "formatted" || text["Formatted"] != tt.want {
				t.Fatalf("got %q, want %q (%s)", text["Formatted"], tt.want, tt.wantKind)
			}
		})
	}
}

func TestFormatErrors(t *testing.T) {
	tests := []struct {
		name     string
		request  func() map[string]any
		body     []byte
		register bool
		wantKind string
	}{
		{"unknown engine", func() map[string]any {
			r := formatRequestFor(1, "a.sh", 1)
			r["Format"].([]any)[0] = "other"
			return r
		}, []byte("x"), true, errKindUnknownEng},
		{"unknown config", func() map[string]any { return formatRequestFor(9, "a.sh", 1) }, []byte("x"), true, errKindUnknownCfg},
		{"length mismatch", func() map[string]any { return formatRequestFor(1, "a.sh", 5) }, []byte("x"), true, errKindProtocol},
		{"invalid utf-8", func() map[string]any { return formatRequestFor(1, "a.sh", 2) }, []byte{0xff, 0xfe}, true, errKindError},
		{"unsupported request", func() map[string]any {
			return map[string]any{"Lint": map[string]any{}}
		}, nil, true, errKindUnsupported},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			s := newServer("1.2.3")
			if tt.register {
				register(t, s, 1, 2, false)
			}
			resp := call(t, s, tt.request(), tt.body)
			raw, ok := resp.Result["Err"]
			if !ok {
				t.Fatalf("want an Err result, got %v", resp.Result)
			}
			var werr wireError
			if err := msgpack.Unmarshal(raw, &werr); err != nil {
				t.Fatal(err)
			}
			if werr.Kind != tt.wantKind {
				t.Errorf("kind = %s, want %s (%s)", werr.Kind, tt.wantKind, werr.Message)
			}
			// An error response leaves the server usable.
			after := call(t, s, formatRequestFor(1, "a.sh", 8), []byte("echo  hi"))
			if _, ok := after.Result["Ok"]; !ok {
				t.Fatalf("server unusable after an error: %v", after.Result)
			}
		})
	}
}

// TestResponsesArePositional pins the v3 encoding: a Response is
// `[result, logs]` and every struct inside it an array, so no field name is on
// the wire.
func TestResponsesArePositional(t *testing.T) {
	failure := newWireError(errKindProtocol, "bad")
	out, err := encodeResponse(nil, failure, []wireLog{{Level: logDebug, Target: "t", Message: "m"}})
	if err != nil {
		t.Fatal(err)
	}
	var decoded []any
	if err := msgpack.Unmarshal(out, &decoded); err != nil {
		t.Fatal(err)
	}
	want := []any{
		map[string]any{"Err": []any{errKindProtocol, "bad", []any{}}},
		[]any{[]any{logDebug, "t", "m"}},
	}
	if got, wantText := fmt.Sprint(decoded), fmt.Sprint(want); got != wantText {
		t.Fatalf("response = %s, want %s", got, wantText)
	}
	for _, name := range []string{"result", "logs", "kind", "message", "chain", "level", "target"} {
		if bytes.Contains(out, []byte(name)) {
			t.Errorf("field name %q is on the wire: %q", name, out)
		}
	}
	init, err := newServer("1.2.3").call(mustPack(t, map[string]any{"Init": []any{uint32(protocolVersion), "x", uint32(1), uint64(1), "Debug"}}), nil)
	if err != nil {
		t.Fatal(err)
	}
	var answer []any
	if err := msgpack.Unmarshal(init, &answer); err != nil {
		t.Fatalf("Init answer %q: %v", init, err)
	}
	want = []any{map[string]any{"Ok": map[string]any{"Init": []any{protocolVersion, "1.2.3"}}}, []any{}}
	if got, wantText := fmt.Sprint(answer), fmt.Sprint(want); got != wantText {
		t.Errorf("Init answer = %s, want %s", got, wantText)
	}
}

func TestDecodeRequestRejectsMalformedHeaders(t *testing.T) {
	tests := []struct {
		name   string
		header []byte
	}{
		{"not msgpack", []byte{0xc1}},
		{"empty map", mustPackNoT(map[string]any{})},
		{"two variants", mustPackNoT(map[string]any{"A": map[string]any{}, "B": map[string]any{}})},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if _, err := decodeRequest(tt.header); err == nil {
				t.Fatal("want an error")
			}
			if _, err := newServer("v").call(tt.header, nil); err == nil {
				t.Fatal("call: want an error for an undecodable header")
			}
		})
	}
}

func mustPackNoT(v any) []byte {
	out, err := msgpack.Marshal(v)
	if err != nil {
		panic(err)
	}
	return out
}

func TestUnpackCall(t *testing.T) {
	pack := func(header, body string) []byte {
		out := make([]byte, 4, 4+len(header)+len(body))
		binary.LittleEndian.PutUint32(out, uint32(len(header))) //nolint:gosec // short test header
		return append(append(out, header...), body...)
	}
	tests := []struct {
		name       string
		payload    []byte
		wantHeader string
		wantBody   string
		wantErr    bool
	}{
		{"header and body", pack("HH", "body"), "HH", "body", false},
		{"empty body", pack("HH", ""), "HH", "", false},
		{"empty header", pack("", "b"), "", "b", false},
		{"too short for length", []byte{1, 0}, "", "", true},
		{"length past end", append([]byte{9, 0, 0, 0}, "abc"...), "", "", true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			header, body, err := unpackCall(tt.payload)
			if (err != nil) != tt.wantErr {
				t.Fatalf("err = %v, wantErr %v", err, tt.wantErr)
			}
			if string(header) != tt.wantHeader || string(body) != tt.wantBody {
				t.Errorf("got %q %q, want %q %q", header, body, tt.wantHeader, tt.wantBody)
			}
		})
	}
}

func TestDescribeMatchesTheEngineContract(t *testing.T) {
	out, err := newServer("9.9.9").describe()
	if err != nil {
		t.Fatal(err)
	}
	var desc wireDescribe
	if err := msgpack.Unmarshal(out, &desc); err != nil {
		t.Fatal(err)
	}
	if desc.Protocol != protocolVersion || desc.ComponentVersion != "9.9.9" || len(desc.Engines) != 1 {
		t.Fatalf("unexpected describe: %+v", desc)
	}
	engine := desc.Engines[0]
	if engine.Name != "shfmt" || !engine.Capabilities.Format || engine.Capabilities.Lint {
		t.Errorf("unexpected engine: %+v", engine)
	}
	if engine.Version == "" || strings.HasPrefix(engine.Version, "v") == false {
		t.Errorf("version should be the module version, got %q", engine.Version)
	}
}

func TestHandleCall(t *testing.T) {
	srv := newServer("1.0.0")
	tests := []struct {
		name       string
		req        envelope
		wantStatus int
	}{
		{"describe method", envelope{Op: "call", Contract: "poly", Method: "describe"}, statusOK},
		{"wrong contract", envelope{Op: "call", Contract: "other", Method: "describe"}, statusFailed},
		{"unknown method", envelope{Op: "call", Contract: "poly", Method: "nope"}, statusFailed},
		{"truncated call payload", envelope{Op: "call", Contract: "poly", Method: "call", Payload: []byte{1}}, statusFailed},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := handleCall(srv, tt.req); got.Status != tt.wantStatus {
				t.Errorf("status = %d, want %d (%s)", got.Status, tt.wantStatus, got.Payload)
			}
		})
	}
}

func TestEnvelopeBytesAreJSONNumberArrays(t *testing.T) {
	out, err := json.Marshal(reply{Status: 0, Payload: []byte{0, 255, 7}})
	if err != nil {
		t.Fatal(err)
	}
	if string(out) != `{"status":0,"payload":[0,255,7]}` {
		t.Fatalf("got %s", out)
	}
	var req envelope
	if err := json.Unmarshal([]byte(`{"op":"call","contract":"poly","method":"call","payload":[1,2]}`), &req); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(req.Payload, []byte{1, 2}) {
		t.Fatalf("payload = %v", req.Payload)
	}
	if err := json.Unmarshal([]byte(`{"payload":[300]}`), &req); err == nil {
		t.Fatal("want an error for a value over 255")
	}
}
