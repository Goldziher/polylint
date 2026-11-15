package main

import (
	"bytes"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"strconv"
)

// maxFrame is the largest frame alef's stdio transport accepts (16 MiB).
const maxFrame = 16 * 1024 * 1024

// readFrame reads one frame: a 4-byte little-endian length, then that many
// bytes of JSON.
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

// writeFrame writes one frame in a single write, so a reader never sees a
// header without its body.
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

// byteArray is a []byte that JSON-encodes as an array of numbers, which is
// how alef's stdio envelopes carry bytes (not base64).
type byteArray []byte

// maxNumberChars is the widest element of the array: `255,`.
const maxNumberChars = 4

// appendByteArray appends src as a JSON array of numbers to dst, without an
// intermediate slice of ints.
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

func (b byteArray) MarshalJSON() ([]byte, error) {
	return appendByteArray(make([]byte, 0, maxNumberChars*len(b)+2), b), nil
}

func (b *byteArray) UnmarshalJSON(data []byte) error {
	out, err := parseByteArray(data)
	if err != nil {
		return err
	}
	*b = out
	return nil
}

// parseByteArray reads a JSON array of integers in 0..=255 straight into bytes.
// `null` is an empty array, as it was when this decoded into a []int.
func parseByteArray(data []byte) ([]byte, error) {
	i := skipSpace(data, 0)
	if bytes.HasPrefix(data[i:], []byte("null")) {
		if skipSpace(data, i+len("null")) != len(data) {
			return nil, errors.New("byte array has trailing data")
		}
		return []byte{}, nil
	}
	if i >= len(data) || data[i] != '[' {
		return nil, errors.New("byte array is not a JSON array")
	}
	i = skipSpace(data, i+1)
	out := make([]byte, 0, len(data)/3+1)
	if i < len(data) && data[i] == ']' {
		return finishByteArray(data, i+1, out)
	}
	for {
		i = skipSpace(data, i)
		start, n := i, 0
		for i < len(data) && data[i] >= '0' && data[i] <= '9' {
			n = n*10 + int(data[i]-'0')
			if n > 255 {
				return nil, errors.New("byte value outside 0..=255")
			}
			i++
		}
		if i == start {
			return nil, fmt.Errorf("byte array element at offset %d is not a non-negative integer", start)
		}
		out = append(out, byte(n))
		i = skipSpace(data, i)
		if i >= len(data) {
			return nil, errors.New("byte array is not terminated")
		}
		switch data[i] {
		case ',':
			i++
		case ']':
			return finishByteArray(data, i+1, out)
		default:
			return nil, fmt.Errorf("unexpected %q in byte array at offset %d", data[i], i)
		}
	}
}

// finishByteArray checks that only whitespace follows the closing bracket.
func finishByteArray(data []byte, end int, out []byte) ([]byte, error) {
	if skipSpace(data, end) != len(data) {
		return nil, errors.New("byte array has trailing data")
	}
	return out, nil
}

// skipSpace returns the index of the first non-whitespace byte at or after i.
func skipSpace(data []byte, i int) int {
	for i < len(data) && (data[i] == ' ' || data[i] == '\t' || data[i] == '\n' || data[i] == '\r') {
		i++
	}
	return i
}

// envelope is a request from the host.
type envelope struct {
	Op       string    `json:"op"`
	Contract string    `json:"contract"`
	Method   string    `json:"method"`
	Payload  byteArray `json:"payload"`
}

// reply is a response to the host. A non-zero Status carries an error message
// as its payload and leaves the connection usable.
type reply struct {
	Status  int       `json:"status"`
	Payload byteArray `json:"payload"`
}

// appendJSON appends the reply as the JSON object alef's stdio envelope expects,
// writing the payload's numbers directly into dst.
func (r reply) appendJSON(dst []byte) []byte {
	if dst == nil {
		dst = make([]byte, 0, maxNumberChars*len(r.Payload)+len(`{"status":3,"payload":[]}`))
	}
	dst = append(dst, `{"status":`...)
	dst = strconv.AppendInt(dst, int64(r.Status), 10)
	dst = append(dst, `,"payload":`...)
	dst = appendByteArray(dst, r.Payload)
	return append(dst, '}')
}
