package main

import (
	"bytes"
	"encoding/binary"
	"errors"
	"io"
	"strings"
	"testing"
)

// The worked example of alef's wire spec: a call with id 1 to engine.echo
// carrying `hi`, and its successful response.
var (
	specCall = []byte{
		0xa2, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
		0x06, 0x00, 'e', 'n', 'g', 'i', 'n', 'e', 'e', 'c', 'h', 'o', 'h', 'i',
	}
	specResponse = []byte{
		0xa2, 0x02, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00,
		0x00, 0x00, 0x00, 0x00, 'h', 'i',
	}
)

func TestReadCallDecodesTheSpecExample(t *testing.T) {
	got, err := readCall(bytes.NewReader(specCall))
	if err != nil {
		t.Fatal(err)
	}
	if got.id != 1 || got.contract != "engine" || got.method != "echo" || string(got.payload) != "hi" {
		t.Fatalf("call = %+v", got)
	}
	if !bytes.Equal(encodeCall(1, "engine", "echo", []byte("hi")), specCall) {
		t.Fatal("the test encoder disagrees with the spec example")
	}
}

func TestResponseFrameEncodesTheSpecExample(t *testing.T) {
	if got := responseFrame(1, reply{Status: statusOK, Payload: []byte("hi")}); !bytes.Equal(got, specResponse) {
		t.Fatalf("response = % x\nwant       % x", got, specResponse)
	}
	failure, err := readAnswer(bytes.NewReader(responseFrame(5, failed("boom"))))
	if err != nil || failure.id != 5 || failure.status != statusFailed || string(failure.payload) != "boom" {
		t.Fatalf("failure = %+v, %v", failure, err)
	}
}

func TestReadCallReadsConsecutiveFramesThenEOF(t *testing.T) {
	in := bytes.NewReader(append(encodeCall(1, "poly", "call", nil), encodeCall(2, "poly", "describe", []byte{0, 255})...))
	for _, want := range []callFrame{{id: 1, contract: "poly", method: "call"}, {id: 2, contract: "poly", method: "describe", payload: []byte{0, 255}}} {
		got, err := readCall(in)
		if err != nil || got.id != want.id || got.method != want.method || !bytes.Equal(got.payload, want.payload) {
			t.Fatalf("call = %+v, %v, want %+v", got, err, want)
		}
	}
	if _, err := readCall(in); !errors.Is(err, io.EOF) {
		t.Fatalf("err = %v, want EOF between frames", err)
	}
}

func TestReadCallRejectsProtocolViolations(t *testing.T) {
	edit := func(change func([]byte) []byte) []byte {
		return change(bytes.Clone(specCall))
	}
	tests := []struct {
		name string
		in   []byte
		want string
	}{
		{"bad magic", edit(func(f []byte) []byte { f[0] = 0xa1; return f }), "magic"},
		{"a response frame", edit(func(f []byte) []byte { f[1] = kindResponse; return f }), "kind"},
		{"an unknown kind", edit(func(f []byte) []byte { f[1] = 9; return f }), "kind"},
		{"flags set", edit(func(f []byte) []byte { f[3] = 1; return f }), "flags"},
		{"request id 0", edit(func(f []byte) []byte { f[4] = 0; return f }), "id is 0"},
		{"header over the limit", edit(func(f []byte) []byte {
			binary.LittleEndian.PutUint32(f[12:], maxHeaderLen+1)
			return f
		}), "header length"},
		{"payload over the limit", edit(func(f []byte) []byte {
			binary.LittleEndian.PutUint32(f[16:], maxPayloadLen+1)
			return f
		}), "payload length"},
		{"contract longer than the header", edit(func(f []byte) []byte { f[20] = 40; return f }), "contract length"},
		{"header too short for a contract length", encodeCallWithHeader([]byte{1}), "no contract length"},
		{"truncated prefix", specCall[:10], "truncated"},
		{"truncated body", specCall[:prefixLen], "truncated"},
		{"truncated payload", specCall[:len(specCall)-1], "truncated"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := readCall(bytes.NewReader(tt.in))
			if err == nil || errors.Is(err, io.EOF) || !strings.Contains(err.Error(), tt.want) {
				t.Fatalf("err = %v, want a protocol error mentioning %q", err, tt.want)
			}
		})
	}
}

// encodeCallWithHeader is a call frame with an arbitrary header.
func encodeCallWithHeader(header []byte) []byte {
	frame := []byte{frameMagic, kindCall, 0, 0}
	frame = binary.LittleEndian.AppendUint64(frame, 1)
	frame = binary.LittleEndian.AppendUint32(frame, uint32(len(header))) //nolint:gosec // a test header is small
	frame = binary.LittleEndian.AppendUint32(frame, 0)
	return append(frame, header...)
}

// Each response is one Write, so concurrent handlers never interleave frames.
func TestResponderWritesWholeFrames(t *testing.T) {
	var writes [][]byte
	w := writerFunc(func(p []byte) (int, error) {
		writes = append(writes, bytes.Clone(p))
		return len(p), nil
	})
	r := &responder{w: w}
	r.respond(1, reply{Status: statusOK, Payload: []byte("hi")})
	r.respond(2, reply{Status: statusOK, Payload: make([]byte, maxPayloadLen+1)})
	if len(writes) != 2 || !bytes.Equal(writes[0], specResponse) {
		t.Fatalf("writes = %d, first % x", len(writes), writes[0])
	}
	oversize, err := readAnswer(bytes.NewReader(writes[1]))
	if err != nil || oversize.id != 2 || oversize.status != statusFailed || !strings.Contains(string(oversize.payload), "frame limit") {
		t.Fatalf("oversize = %+v, %v, want a frame-limit failure", oversize, err)
	}
	if r.failure() != nil {
		t.Fatal(r.failure())
	}
}

func TestResponderKeepsTheFirstWriteError(t *testing.T) {
	calls := 0
	r := &responder{w: writerFunc(func([]byte) (int, error) {
		calls++
		return 0, io.ErrClosedPipe
	})}
	r.respond(1, reply{})
	r.respond(2, reply{})
	if calls != 1 || !errors.Is(r.failure(), io.ErrClosedPipe) {
		t.Fatalf("calls = %d, failure = %v", calls, r.failure())
	}
}

type writerFunc func([]byte) (int, error)

func (f writerFunc) Write(p []byte) (int, error) { return f(p) }
