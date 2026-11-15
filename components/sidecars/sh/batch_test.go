package main

import (
	"bytes"
	"fmt"
	"strings"
	"testing"

	"github.com/vmihailenco/msgpack/v5"
)

// batchInput is one member of a test batch.
type batchInput struct {
	path string
	src  []byte
}

// outcome is a normalized member result: kind is "unchanged", "formatted" or
// "err"; text is the formatted source or the error kind.
type outcome struct{ kind, text string }

func decodeOutput(t *testing.T, raw msgpack.RawMessage) outcome {
	t.Helper()
	var unchanged string
	if err := msgpack.Unmarshal(raw, &unchanged); err == nil {
		if unchanged != "Unchanged" {
			t.Fatalf("unexpected output %q", unchanged)
		}
		return outcome{kind: "unchanged"}
	}
	var text map[string]string
	if err := msgpack.Unmarshal(raw, &text); err != nil {
		t.Fatal(err)
	}
	return outcome{kind: "formatted", text: text["Formatted"]}
}

// perFile formats one input through the single-file request.
func perFile(t *testing.T, s *server, in batchInput) outcome {
	t.Helper()
	resp := call(t, s, formatRequestFor(1, in.path, len(in.src)), in.src)
	ok, found := resp.Result["Ok"]
	if !found {
		var werr wireError
		if err := msgpack.Unmarshal(resp.Result["Err"], &werr); err != nil {
			t.Fatal(err)
		}
		return outcome{kind: "err", text: werr.Kind}
	}
	var payload map[string]msgpack.RawMessage
	if err := msgpack.Unmarshal(ok, &payload); err != nil {
		t.Fatal(err)
	}
	return decodeOutput(t, payload["Formatted"])
}

// batchRequestFor packs inputs back to back, with gap bytes of padding between
// members that no member covers.
func batchRequestFor(id uint32, inputs []batchInput, gap int) (map[string]any, []byte) {
	var body []byte
	files := make([]any, 0, len(inputs))
	for _, in := range inputs {
		body = append(body, bytes.Repeat([]byte{0xff}, gap)...)
		files = append(files, []any{
			in.path, "shell", uint64(len(body)), uint64(len(in.src)), //nolint:gosec // test sizes are small
		})
		body = append(body, in.src...)
	}
	return map[string]any{"FormatBatch": []any{"shfmt", id, files}}, body
}

// batch formats inputs through FormatBatch and returns each member's outcome.
func batch(t *testing.T, s *server, inputs []batchInput, gap int) ([]outcome, response) {
	t.Helper()
	request, body := batchRequestFor(1, inputs, gap)
	resp := call(t, s, request, body)
	ok, found := resp.Result["Ok"]
	if !found {
		return nil, resp
	}
	var payload map[string]msgpack.RawMessage
	if err := msgpack.Unmarshal(ok, &payload); err != nil {
		t.Fatal(err)
	}
	var items []map[string]msgpack.RawMessage
	if err := msgpack.Unmarshal(payload["FormatBatch"], &items); err != nil {
		t.Fatal(err)
	}
	out := make([]outcome, len(items))
	for i, item := range items {
		if raw, isOK := item["Ok"]; isOK {
			out[i] = decodeOutput(t, raw)
			continue
		}
		var werr wireError
		if err := msgpack.Unmarshal(item["Err"], &werr); err != nil {
			t.Fatal(err)
		}
		out[i] = outcome{kind: "err", text: werr.Kind}
	}
	return out, resp
}

func TestBatchSupportIsAdvertised(t *testing.T) {
	s := newServer("1.2.3")
	register(t, s, 1, 2, false)
	resp := call(t, s, map[string]any{"BatchSupport": []any{"shfmt", uint32(1)}}, nil)
	ok, found := resp.Result["Ok"]
	if !found {
		t.Fatalf("not ok: %v", resp.Result)
	}
	var payload map[string]struct {
		Format bool `msgpack:"format"`
		Lint   bool `msgpack:"lint"`
	}
	if err := msgpack.Unmarshal(ok, &payload); err != nil {
		t.Fatal(err)
	}
	if got := payload["BatchSupport"]; !got.Format || got.Lint {
		t.Fatalf("batch support = %+v, want format only", got)
	}
	unknown := call(t, s, map[string]any{"BatchSupport": []any{"other", uint32(1)}}, nil)
	if _, found := unknown.Result["Err"]; !found {
		t.Fatalf("an unknown engine must be an error, got %v", unknown.Result)
	}

	out, err := s.describe()
	if err != nil {
		t.Fatal(err)
	}
	var desc wireDescribe
	if err := msgpack.Unmarshal(out, &desc); err != nil {
		t.Fatal(err)
	}
	if desc.Engines[0].Hooks&hookBatchSupport == 0 {
		t.Errorf("hooks = %#x: the host never asks BatchSupport without bit %#x", desc.Engines[0].Hooks, hookBatchSupport)
	}
}

func TestFormatBatchEqualsPerFileFormatting(t *testing.T) {
	s := newServer("1.2.3")
	register(t, s, 1, 2, false)
	inputs := append(append([]batchInput{}, []batchInput{
		{"basic.sh", []byte("greet() {\n        echo hi\n}\n")},
		{"already.sh", []byte("echo hi\n")},
		{"broken.sh", []byte("if [ 1 ]; then\n")},
		{"zsh.zsh", []byte("if [[ -n $x ]]; then\n        echo   $x\nfi\n")},
		{"bad.sh", []byte{0xff, 0xfe}},
	}...), batchInput{"empty.sh", nil})
	for _, gap := range []int{0, 3} {
		got, resp := batch(t, s, inputs, gap)
		if got == nil {
			t.Fatalf("batch failed: %v", resp.Result)
		}
		if len(got) != len(inputs) {
			t.Fatalf("%d results for %d members", len(got), len(inputs))
		}
		for i, in := range inputs {
			if want := perFile(t, s, in); got[i] != want {
				t.Errorf("gap %d, %s: batch %+v, per-file %+v", gap, in.path, got[i], want)
			}
		}
	}
}

// Every member has a distinct result, so an ordering mistake in the worker pool
// shows as one member carrying another's output.
func TestFormatBatchKeepsMemberOrder(t *testing.T) {
	s := newServer("1.2.3")
	register(t, s, 1, 2, false)
	inputs := make([]batchInput, 200)
	for i := range inputs {
		inputs[i] = batchInput{fmt.Sprintf("m%d.sh", i), []byte(fmt.Sprintf("echo   n%d\n", i))}
	}
	got, resp := batch(t, s, inputs, 1)
	if got == nil {
		t.Fatalf("batch failed: %v", resp.Result)
	}
	for i, in := range inputs {
		if want := perFile(t, s, in); got[i] != want || got[i].kind != "formatted" || !strings.Contains(got[i].text, fmt.Sprint(i)) {
			t.Fatalf("member %d: batch %+v, per-file %+v", i, got[i], want)
		}
	}
}

func TestFormatBatchMemberFailuresAreIsolated(t *testing.T) {
	s := newServer("1.2.3")
	register(t, s, 1, 2, false)
	inner := s.format
	s.format = func(path string, src []byte, opts formatOptions) (string, bool, error) {
		if strings.Contains(string(src), "exploding member") {
			panic("printer exploded")
		}
		return inner(path, src, opts)
	}
	inputs := []batchInput{
		{"a.sh", []byte("echo   hi\n")},
		{"bad.sh", []byte{0xff, 0xfe}},
		{"boom.sh", []byte("exploding member\n")},
		{"b.sh", []byte("echo   hi\n")},
	}
	got, resp := batch(t, s, inputs, 0)
	if got == nil {
		t.Fatalf("one bad member must not fail the batch: %v", resp.Result)
	}
	want := []outcome{{"formatted", ""}, {"err", errKindError}, {"err", errKindError}, {"formatted", ""}}
	for i := range want {
		if got[i].kind != want[i].kind || (want[i].kind == "err" && got[i].text != want[i].text) {
			t.Errorf("member %d = %+v, want %+v", i, got[i], want[i])
		}
	}
	if got[0] != got[3] {
		t.Errorf("identical members differ: %+v vs %+v", got[0], got[3])
	}
}

func TestFormatBatchRejectsMalformedRequests(t *testing.T) {
	s := newServer("1.2.3")
	register(t, s, 1, 2, false)
	file := func(offset, n uint64) []any {
		return []any{"a.sh", "shell", offset, n}
	}
	tests := []struct {
		name     string
		engine   string
		config   uint32
		files    []any
		wantKind string
	}{
		{"unknown engine", "other", 1, nil, errKindUnknownEng},
		{"unknown config", "shfmt", 9, nil, errKindUnknownCfg},
		{"slice past the body", "shfmt", 1, []any{file(2, 9)}, errKindProtocol},
		{"offset past the body", "shfmt", 1, []any{file(9, 0)}, errKindProtocol},
		{"offset and length overflow", "shfmt", 1, []any{file(1, ^uint64(0))}, errKindProtocol},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			files := tt.files
			if files == nil {
				files = []any{}
			}
			resp := call(t, s, map[string]any{"FormatBatch": []any{tt.engine, tt.config, files}}, []byte("12345"))
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
		})
	}
	if got, resp := batch(t, s, nil, 0); got == nil || len(got) != 0 {
		t.Fatalf("an empty batch answers an empty result list, got %v %v", got, resp.Result)
	}
}
