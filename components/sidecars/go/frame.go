package main

import (
	"encoding/binary"
	"fmt"
	"io"
	"strconv"
)

// maxFrame is the largest protocol-1 frame alef's stdio transport accepts
// (16 MiB). Only the describe handshake travels in protocol-1 frames.
const maxFrame = 16 * 1024 * 1024

// readFrame reads one protocol-1 frame: a 4-byte little-endian length, then
// that many bytes of JSON.
func readFrame(r io.Reader) ([]byte, error) {
	var header [4]byte
	if _, err := io.ReadFull(r, header[:]); err != nil {
		return nil, err
	}
	n := binary.LittleEndian.Uint32(header[:])
	if n == 0 || n > maxFrame {
		return nil, fmt.Errorf("frame length %d is outside 1..=%d", n, maxFrame)
	}
	body := make([]byte, n)
	if _, err := io.ReadFull(r, body); err != nil {
		return nil, fmt.Errorf("read frame body: %w", err)
	}
	return body, nil
}

// writeFrame writes one protocol-1 frame in a single write, so a reader never
// sees a header without its body.
func writeFrame(w io.Writer, body []byte) error {
	if len(body) == 0 || len(body) > maxFrame {
		return fmt.Errorf("frame length %d is outside 1..=%d", len(body), maxFrame)
	}
	frame := make([]byte, 4+len(body))
	binary.LittleEndian.PutUint32(frame, uint32(len(body))) //nolint:gosec // len(body) is checked against maxFrame above
	copy(frame[4:], body)
	if _, err := w.Write(frame); err != nil {
		return fmt.Errorf("write frame: %w", err)
	}
	return nil
}

// appendByteArray appends src as a JSON array of numbers to dst, which is how
// a protocol-1 envelope carries bytes (not base64).
func appendByteArray(dst, src []byte) []byte {
	dst = append(dst, '[')
	for i, v := range src {
		if i > 0 {
			dst = append(dst, ',')
		}
		dst = strconv.AppendUint(dst, uint64(v), 10)
	}
	return append(dst, ']')
}

// envelope is the protocol-1 describe request. Fields it does not name are
// ignored, as the protocol requires.
type envelope struct {
	Op string `json:"op"`
}

// reply is the answer to one request: a zero Status carries the result as its
// payload, a non-zero one an error message, which leaves the connection usable.
type reply struct {
	Status  int32
	Payload []byte
}

// appendJSON appends the reply as the protocol-1 JSON envelope the describe
// handshake answers with.
func (r reply) appendJSON(dst []byte) []byte {
	dst = append(dst, `{"status":`...)
	dst = strconv.AppendInt(dst, int64(r.Status), 10)
	dst = append(dst, `,"payload":`...)
	dst = appendByteArray(dst, r.Payload)
	return append(dst, '}')
}
