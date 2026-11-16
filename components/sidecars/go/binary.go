package main

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"runtime"
	"strings"
	"sync"
)

// Stdio protocol 2, the binary frames alef's host speaks after the describe
// handshake (alef docs/design/component-stdio-binary-protocol.md): a 20-byte
// little-endian prefix, a kind-specific header, then the payload as raw bytes.
const (
	// stdioProtocolsEnv lists the stdio protocol versions the host speaks.
	stdioProtocolsEnv = "ALEF_STDIO_PROTOCOLS"
	binaryProtocol    = 2

	frameMagic   = 0xA2
	kindCall     = 1
	kindResponse = 2
	prefixLen    = 20
	// maxHeaderLen and maxPayloadLen are the protocol's limits; the payload
	// limit applies to the raw bytes.
	maxHeaderLen  = 64 * 1024
	maxPayloadLen = 16 * 1024 * 1024
	// responseHeaderLen is the i32 status every response header holds.
	responseHeaderLen = 4
)

// offersBinary reports whether the host's ALEF_STDIO_PROTOCOLS list names
// protocol 2.
func offersBinary(protocols string) bool {
	for _, version := range strings.Split(protocols, ",") {
		if strings.TrimSpace(version) == "2" {
			return true
		}
	}
	return false
}

// maxInFlight is how many calls this process accepts at once: one per CPU,
// since formatting is CPU-bound.
func maxInFlight() uint32 {
	return uint32(max(runtime.GOMAXPROCS(0), 1)) //nolint:gosec // GOMAXPROCS is a small positive count
}

// callFrame is one decoded protocol-2 call.
type callFrame struct {
	id       uint64
	contract string
	method   string
	payload  []byte
}

// readCall reads one call frame. The end of input between frames is io.EOF;
// a frame that breaks the protocol's rules, including one cut short, is an
// error that ends the connection.
func readCall(r io.Reader) (callFrame, error) {
	var prefix [prefixLen]byte
	if _, err := io.ReadFull(r, prefix[:]); err != nil {
		if errors.Is(err, io.ErrUnexpectedEOF) {
			return callFrame{}, errors.New("truncated frame prefix")
		}
		return callFrame{}, err
	}
	if prefix[0] != frameMagic {
		return callFrame{}, fmt.Errorf("frame magic is %#x, want %#x", prefix[0], frameMagic)
	}
	if prefix[1] != kindCall {
		return callFrame{}, fmt.Errorf("frame kind is %d, want a call (%d)", prefix[1], kindCall)
	}
	if flags := binary.LittleEndian.Uint16(prefix[2:4]); flags != 0 {
		return callFrame{}, fmt.Errorf("frame flags are %#x, want 0", flags)
	}
	id := binary.LittleEndian.Uint64(prefix[4:12])
	if id == 0 {
		return callFrame{}, errors.New("frame request id is 0")
	}
	headerLen := binary.LittleEndian.Uint32(prefix[12:16])
	payloadLen := binary.LittleEndian.Uint32(prefix[16:20])
	if headerLen > maxHeaderLen {
		return callFrame{}, fmt.Errorf("call %d: header length %d exceeds %d", id, headerLen, maxHeaderLen)
	}
	if payloadLen > maxPayloadLen {
		return callFrame{}, fmt.Errorf("call %d: payload length %d exceeds %d", id, payloadLen, maxPayloadLen)
	}
	body := make([]byte, int(headerLen)+int(payloadLen))
	if _, err := io.ReadFull(r, body); err != nil {
		// Not wrapped: an EOF here is a truncated frame, never a clean end.
		return callFrame{}, fmt.Errorf("call %d: truncated frame: %v", id, err)
	}
	header := body[:headerLen]
	if len(header) < 2 {
		return callFrame{}, fmt.Errorf("call %d: header of %d bytes has no contract length", id, len(header))
	}
	contractLen := int(binary.LittleEndian.Uint16(header))
	if 2+contractLen > len(header) {
		return callFrame{}, fmt.Errorf("call %d: contract length %d exceeds the %d-byte header", id, contractLen, len(header)-2)
	}
	return callFrame{
		id:       id,
		contract: string(header[2 : 2+contractLen]),
		method:   string(header[2+contractLen:]),
		payload:  body[headerLen:],
	}, nil
}

// responseFrame encodes the response to call id: the prefix, the status and
// the payload, as one buffer.
func responseFrame(id uint64, answer reply) []byte {
	frame := make([]byte, prefixLen+responseHeaderLen, prefixLen+responseHeaderLen+len(answer.Payload))
	frame[0] = frameMagic
	frame[1] = kindResponse
	binary.LittleEndian.PutUint64(frame[4:12], id)
	binary.LittleEndian.PutUint32(frame[12:16], responseHeaderLen)
	binary.LittleEndian.PutUint32(frame[16:20], uint32(len(answer.Payload))) //nolint:gosec // the responder bounds it by maxPayloadLen
	binary.LittleEndian.PutUint32(frame[20:24], uint32(answer.Status))       //nolint:gosec // the i32 status travels as its two's-complement bits
	return append(frame, answer.Payload...)
}

// responder writes the responses of concurrent calls. Each frame is one Write
// under the lock, so frames never interleave on stdout.
type responder struct {
	mu  sync.Mutex
	w   io.Writer
	err error
}

// respond answers call id. A payload over the frame limit would make the host
// drop the connection and fail every call in flight, so it becomes this call's
// error instead.
func (r *responder) respond(id uint64, answer reply) {
	if len(answer.Payload) > maxPayloadLen {
		answer = failed("reply of %d bytes exceeds the %d-byte frame limit", len(answer.Payload), maxPayloadLen)
	}
	frame := responseFrame(id, answer)
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.err != nil {
		return
	}
	if _, err := r.w.Write(frame); err != nil {
		r.err = fmt.Errorf("write response %d: %w", id, err)
	}
}

// failure is the first write error, if any.
func (r *responder) failure() error {
	r.mu.Lock()
	defer r.mu.Unlock()
	return r.err
}
