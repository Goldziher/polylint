package main

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"errors"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func testManifest() componentManifest {
	var m componentManifest
	m.Identity.Component = "go"
	m.Identity.Version = "1.2.3"
	m.Identity.FeatureHash = "ab"
	m.Provides = []providedContract{{Contract: "poly", InterfaceVersion: 1, ContractHash: "cd", Implementation: "NewGoComponent"}}
	return m
}

// registerCall registers config with the default options.
func registerCall(t *testing.T, id uint64, config uint32) []byte {
	t.Helper()
	return polyCall(t, id, map[string]any{"RegisterConfig": []any{
		config, wireTab("indent_width", wireInt(8), "options", wireTab()),
	}}, "")
}

// formatCall formats src as run.go under config.
func formatCall(t *testing.T, id uint64, config uint32, src string) []byte {
	t.Helper()
	return polyCall(t, id, formatRequestFor(config, "run.go", len(src)), src)
}

// sampleSource is a file the formatter changes; tag makes it distinguishable.
func sampleSource(tag string) string { return "package p\nvar   " + tag + " = 1\n" }

func TestHandshakeSelectsProtocol2(t *testing.T) {
	var out bytes.Buffer
	if err := run(bytes.NewReader(describeRequest(t)), &out, testManifest(), "1,2"); !errors.Is(err, io.EOF) {
		t.Fatalf("err = %v, want EOF at the end of input", err)
	}
	status, payload := readHandshake(t, &out)
	if status != statusOK {
		t.Fatalf("describe = status %d %q", status, payload)
	}
	var desc stdioDescriptor
	if err := json.Unmarshal(payload, &desc); err != nil {
		t.Fatal(err)
	}
	want := testManifest().descriptor(maxInFlight())
	if desc.ProtocolVersion != binaryProtocol || desc.MaxInFlight < 1 || desc.MaxInFlight != want.MaxInFlight {
		t.Errorf("protocol %d, max_in_flight %d, want protocol 2 and %d", desc.ProtocolVersion, desc.MaxInFlight, want.MaxInFlight)
	}
	if desc.ComponentID != want.ComponentID || desc.ComponentVer != "1.2.3" || desc.FeatureSetHash != "ab" ||
		len(desc.Provides) != 1 || desc.Provides[0] != want.Provides[0] {
		t.Errorf("descriptor = %+v", desc)
	}
	if out.Len() != 0 {
		t.Fatalf("%d bytes written after the describe reply", out.Len())
	}
}

// Without protocol 2 the host would send protocol-1 calls this sidecar does not
// read, so it fails the handshake instead of declaring protocol 1.
func TestHandshakeRefusesAHostWithoutProtocol2(t *testing.T) {
	for _, protocols := range []string{"", "1", "1,3"} {
		t.Run(protocols, func(t *testing.T) {
			var out bytes.Buffer
			err := run(bytes.NewReader(describeRequest(t)), &out, testManifest(), protocols)
			if err == nil || errors.Is(err, io.EOF) {
				t.Fatalf("err = %v, want a refusal", err)
			}
			status, payload := readHandshake(t, &out)
			if status != statusFailed || !strings.Contains(string(payload), "protocol 2") {
				t.Fatalf("describe = status %d %q, want a failure naming protocol 2", status, payload)
			}
		})
	}
}

func TestOffersBinary(t *testing.T) {
	for protocols, want := range map[string]bool{"1,2": true, "2": true, " 1 , 2 ": true, "": false, "1": false, "12": false} {
		if got := offersBinary(protocols); got != want {
			t.Errorf("offersBinary(%q) = %v, want %v", protocols, got, want)
		}
	}
}

func TestRunRejectsBrokenInput(t *testing.T) {
	frame := func(body string) []byte {
		var buf bytes.Buffer
		if err := writeFrame(&buf, []byte(body)); err != nil {
			t.Fatal(err)
		}
		return buf.Bytes()
	}
	afterDescribe := func(call []byte) []byte { return append(describeRequest(t), call...) }
	badMagic := encodeCall(1, contractPoly, methodDescr, nil)
	badMagic[0] = 0xA1
	tests := []struct {
		name string
		in   []byte
	}{
		{"not json", frame(`nope`)},
		{"a call before describe", frame(`{"op":"call","contract":"poly","method":"describe","payload":[]}`)},
		{"zero-length frame", binary.LittleEndian.AppendUint32(nil, 0)},
		{"a bad call frame", afterDescribe(badMagic)},
		{"a truncated call frame", afterDescribe(encodeCall(1, contractPoly, methodDescr, []byte("abc"))[:prefixLen+3])},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := run(bytes.NewReader(tt.in), io.Discard, testManifest(), "1,2")
			if err == nil || errors.Is(err, io.EOF) {
				t.Fatalf("err = %v, want a protocol error", err)
			}
		})
	}
}

func TestCallsAreAnsweredByID(t *testing.T) {
	s := startSession(t, newServer("1.2.3"))
	if got := s.call(7, encodeCall(7, contractPoly, "nope", nil)); got.status != statusFailed {
		t.Fatalf("a bad method must be a status error: %d %q", got.status, got.payload)
	}
	if got := s.call(9, encodeCall(9, contractPoly, methodDescr, nil)); got.status != statusOK {
		t.Fatalf("describe method: %d %q", got.status, got.payload)
	}
	if err := s.finish(); !errors.Is(err, io.EOF) {
		t.Fatalf("err = %v, want EOF", err)
	}
}

// A call that is still running must not hold up a later one: the first
// formatter blocks until the test has read the second call's answer.
func TestCallsRunConcurrently(t *testing.T) {
	srv := newServer("1.2.3")
	release := make(chan struct{})
	srv.format = func(path string, src []byte, opts formatOptions) (string, bool, error) {
		if strings.Contains(string(src), "first") {
			select {
			case <-release:
			case <-time.After(5 * time.Second):
				return "", false, errors.New("never released: calls are serialized")
			}
		}
		return formatSource(path, src, opts)
	}
	s := startSession(t, srv)
	if got := s.call(1, registerCall(t, 1, 1)); got.status != statusOK {
		t.Fatalf("RegisterConfig: %d %q", got.status, got.payload)
	}
	first, second := formatCall(t, 2, 1, sampleSource("first")), formatCall(t, 3, 1, sampleSource("second"))
	// Sent from a goroutine, as the host's writer thread does, so a sidecar that
	// serializes calls fails this test instead of deadlocking it.
	go func() {
		_, _ = s.in.Write(first)
		_, _ = s.in.Write(second)
	}()
	if got := s.next(); got.id != 3 || got.status != statusOK {
		t.Fatalf("the first answer is for call %d (%q), want call 3 while call 2 still runs", got.id, got.payload)
	}
	close(release)
	if got := s.next(); got.id != 2 || got.status != statusOK {
		t.Fatalf("the second answer is for call %d (%q), want call 2", got.id, got.payload)
	}
	if err := s.finish(); !errors.Is(err, io.EOF) {
		t.Fatalf("err = %v", err)
	}
}

// A reply over the frame limit would make the host drop the connection; it must
// be a per-call error and the next request must still be served.
func TestRunAnswersAnOversizeReplyWithAnErrorAndKeepsServing(t *testing.T) {
	srv := newServer("1.2.3")
	huge := strings.Repeat("e", maxPayloadLen+1024)
	srv.format = func(path string, src []byte, opts formatOptions) (string, bool, error) {
		if strings.Contains(string(src), "huge") {
			return huge, true, nil
		}
		return formatSource(path, src, opts)
	}
	s := startSession(t, srv)
	if got := s.call(1, registerCall(t, 1, 1)); got.status != statusOK {
		t.Fatalf("RegisterConfig status = %d", got.status)
	}
	big := s.call(2, formatCall(t, 2, 1, sampleSource("huge")))
	if big.status != statusFailed || !strings.Contains(string(big.payload), "frame limit") {
		t.Fatalf("oversize reply = status %d %q, want a frame-limit error", big.status, big.payload)
	}
	if got := s.call(3, formatCall(t, 3, 1, sampleSource("small"))); got.status != statusOK {
		t.Fatalf("the call after the oversize one: status %d %q", got.status, got.payload)
	}
	if err := s.finish(); !errors.Is(err, io.EOF) {
		t.Fatalf("err = %v, want EOF after serving every frame", err)
	}
}

// Payload bytes travel raw, so a file well past protocol 1's 4 MiB ceiling fits
// one frame, and the real formatter must take it.
func TestRunFormatsALargeFile(t *testing.T) {
	src := "package p\n\n// " + strings.Repeat("e", 12*1024*1024) + "\n"
	s := startSession(t, newServer("1.2.3"))
	s.call(1, registerCall(t, 1, 1))
	if got := s.call(2, formatCall(t, 2, 1, src)); got.status != statusOK {
		t.Fatalf("status %d %q", got.status, got.payload)
	}
	if err := s.finish(); !errors.Is(err, io.EOF) {
		t.Fatalf("err = %v", err)
	}
}

func TestHandleCallRecoversFromAPanic(t *testing.T) {
	srv := newServer("1.2.3")
	srv.format = func(string, []byte, formatOptions) (string, bool, error) { panic("printer exploded") }
	s := startSession(t, srv)
	s.call(1, registerCall(t, 1, 1))
	for id := uint64(2); id <= 3; id++ {
		got := s.call(id, formatCall(t, id, 1, sampleSource("a")))
		if got.status != statusFailed || !strings.Contains(string(got.payload), "printer exploded") {
			t.Fatalf("panicking call = status %d %q, want a failed reply naming the panic", got.status, got.payload)
		}
	}
	if err := s.finish(); !errors.Is(err, io.EOF) {
		t.Fatalf("err = %v, want EOF: a panic must not end the process", err)
	}
}

func TestReadManifest(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, manifestFile)
	if _, err := readManifest(path); err == nil {
		t.Fatal("want an error for a missing manifest")
	}
	doc := `{"identity":{"component":"go","version":"0.9.0","feature_hash":"ff"},"provides":[{"contract":"poly","interface_version":1,"contract_hash":"aa","implementation":"X"}]}`
	if err := os.WriteFile(path, []byte(doc), 0o600); err != nil {
		t.Fatal(err)
	}
	m, err := readManifest(path)
	if err != nil {
		t.Fatal(err)
	}
	if m.Identity.Version != "0.9.0" || m.Provides[0].ContractHash != "aa" {
		t.Errorf("manifest = %+v", m)
	}
}
