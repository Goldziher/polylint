package main

import (
	"bufio"
	"bytes"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"io"
	"testing"
	"time"
)

// describeRequest is the protocol-1 handshake frame every connection opens with.
func describeRequest(t *testing.T) []byte {
	t.Helper()
	var buf bytes.Buffer
	if err := writeFrame(&buf, []byte(`{"op":"describe"}`)); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

// encodeCall is a protocol-2 call frame, built here from the wire spec rather
// than with the code under test.
func encodeCall(id uint64, contract, method string, payload []byte) []byte {
	header := binary.LittleEndian.AppendUint16(nil, uint16(len(contract))) //nolint:gosec // test names are short
	header = append(header, contract...)
	header = append(header, method...)
	frame := []byte{frameMagic, kindCall, 0, 0}
	frame = binary.LittleEndian.AppendUint64(frame, id)
	frame = binary.LittleEndian.AppendUint32(frame, uint32(len(header)))  //nolint:gosec // a test header is small
	frame = binary.LittleEndian.AppendUint32(frame, uint32(len(payload))) //nolint:gosec // a test payload is within the frame limit
	frame = append(frame, header...)
	return append(frame, payload...)
}

// polyCall is a `call` method frame carrying a MessagePack request and a body.
func polyCall(t *testing.T, id uint64, request map[string]any, body string) []byte {
	t.Helper()
	header := mustPack(t, request)
	payload := binary.LittleEndian.AppendUint32(nil, uint32(len(header))) //nolint:gosec // a test header is small
	payload = append(payload, header...)
	payload = append(payload, body...)
	return encodeCall(id, contractPoly, methodCall, payload)
}

// answer is one decoded protocol-2 response frame.
type answer struct {
	id      uint64
	status  int32
	payload []byte
}

// readAnswer decodes one response frame, checking every fixed field.
func readAnswer(r io.Reader) (answer, error) {
	var prefix [prefixLen]byte
	if _, err := io.ReadFull(r, prefix[:]); err != nil {
		return answer{}, err
	}
	if prefix[0] != frameMagic || prefix[1] != kindResponse || prefix[2] != 0 || prefix[3] != 0 {
		return answer{}, fmt.Errorf("bad response prefix % x", prefix[:4])
	}
	if n := binary.LittleEndian.Uint32(prefix[12:16]); n != responseHeaderLen {
		return answer{}, fmt.Errorf("response header length is %d, want %d", n, responseHeaderLen)
	}
	rest := make([]byte, responseHeaderLen+int(binary.LittleEndian.Uint32(prefix[16:20])))
	if _, err := io.ReadFull(r, rest); err != nil {
		return answer{}, fmt.Errorf("truncated response: %w", err)
	}
	return answer{
		id:      binary.LittleEndian.Uint64(prefix[4:12]),
		status:  int32(binary.LittleEndian.Uint32(rest)), //nolint:gosec // the status is an i32 on the wire
		payload: rest[responseHeaderLen:],
	}, nil
}

// readHandshake decodes the protocol-1 describe reply.
func readHandshake(t *testing.T, r io.Reader) (int32, []byte) {
	t.Helper()
	raw, err := readFrame(r)
	if err != nil {
		t.Fatalf("no describe reply: %v", err)
	}
	var decoded struct {
		Status  int32 `json:"status"`
		Payload []int `json:"payload"`
	}
	if err := json.Unmarshal(raw, &decoded); err != nil {
		t.Fatalf("describe reply %s: %v", raw, err)
	}
	payload := make([]byte, len(decoded.Payload))
	for i, v := range decoded.Payload {
		payload[i] = byte(v) //nolint:gosec // the reply holds byte values
	}
	return decoded.Status, payload
}

// session drives serve over pipes the way the host does, so a test can wait for
// one answer before it sends the next call: calls run concurrently, and a
// Format sent before its RegisterConfig was answered may overtake it.
type session struct {
	t    *testing.T
	in   *io.PipeWriter
	out  *bufio.Reader
	done chan error
}

// startSession runs serve on srv and completes the describe handshake.
func startSession(t *testing.T, srv *server) *session {
	t.Helper()
	inR, inW := io.Pipe()
	outR, outW := io.Pipe()
	s := &session{t: t, in: inW, out: bufio.NewReader(outR), done: make(chan error, 1)}
	go func() {
		err := serve(inR, outW, testManifest(), srv, "1,2")
		_ = inR.CloseWithError(err)
		_ = outW.Close()
		s.done <- err
	}()
	t.Cleanup(func() {
		_ = inW.Close()
		_ = outR.Close()
	})
	s.send(describeRequest(t))
	if status, payload := readHandshake(t, s.out); status != statusOK {
		t.Fatalf("describe = status %d %q", status, payload)
	}
	return s
}

func (s *session) send(frame []byte) {
	s.t.Helper()
	if _, err := s.in.Write(frame); err != nil {
		s.t.Fatalf("send: %v", err)
	}
}

func (s *session) next() answer {
	s.t.Helper()
	got, err := readAnswer(s.out)
	if err != nil {
		s.t.Fatalf("no answer: %v", err)
	}
	return got
}

// call sends one frame and waits for the answer, which must carry id.
func (s *session) call(id uint64, frame []byte) answer {
	s.t.Helper()
	s.send(frame)
	got := s.next()
	if got.id != id {
		s.t.Fatalf("answer id = %d, want %d", got.id, id)
	}
	return got
}

// finish closes the input and returns serve's result, failing if anything is
// answered after the calls the test read.
func (s *session) finish() error {
	s.t.Helper()
	_ = s.in.Close()
	if extra, err := readAnswer(s.out); err == nil {
		s.t.Fatalf("an extra answer for call %d", extra.id)
	}
	select {
	case err := <-s.done:
		return err
	case <-time.After(30 * time.Second):
		s.t.Fatal("serve did not return after the input closed")
		return nil
	}
}
