// Command sh is poly's shell formatter component: shfmt (mvdan.cc/sh) behind
// alef's stdio transport. It runs as one long-lived child of poly: after the
// describe handshake it reads binary call frames (stdio protocol 2) on stdin,
// serves them concurrently, answers on stdout and logs to stderr.
package main

import (
	"bufio"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"runtime/debug"
	"sync"
)

const (
	// statusOK and statusFailed are alef's AlefStatus codes.
	statusOK     = 0
	statusFailed = 3

	contractPoly = "poly"
	methodDescr  = "describe"
	methodCall   = "call"
)

func main() {
	manifest, err := loadManifest()
	if err == nil {
		err = run(os.Stdin, os.Stdout, manifest, os.Getenv(stdioProtocolsEnv))
	}
	if err != nil && !errors.Is(err, io.EOF) {
		fmt.Fprintf(os.Stderr, "poly-sh: %v\n", err)
		os.Exit(1)
	}
}

// run serves the connection until the host closes stdin. protocols is the
// host's ALEF_STDIO_PROTOCOLS.
func run(in io.Reader, out io.Writer, manifest componentManifest, protocols string) error {
	return serve(in, out, manifest, newServer(manifest.Identity.Version), protocols)
}

// serve is run over a given server, which tests use to inject a formatter.
func serve(in io.Reader, out io.Writer, manifest componentManifest, srv *server, protocols string) error {
	reader := bufio.NewReader(in)
	if err := handshake(reader, out, manifest, protocols); err != nil {
		return err
	}
	return serveCalls(reader, out, srv)
}

// handshake answers the protocol-1 describe frame every connection starts
// with, selecting protocol 2. This sidecar speaks nothing else afterwards, so a
// host that does not offer protocol 2 gets a failed describe.
func handshake(in io.Reader, out io.Writer, manifest componentManifest, protocols string) error {
	frame, err := readFrame(in)
	if err != nil {
		return err
	}
	var req envelope
	if err := json.Unmarshal(frame, &req); err != nil {
		return fmt.Errorf("decode describe request: %w", err)
	}
	if req.Op != "describe" {
		return fmt.Errorf("the first request is op %q, want describe", req.Op)
	}
	if !offersBinary(protocols) {
		answer := failed("this sidecar needs stdio protocol %d; the host offers %q", binaryProtocol, protocols)
		if err := writeFrame(out, answer.appendJSON(nil)); err != nil {
			return err
		}
		return fmt.Errorf("the host does not offer stdio protocol %d (%s=%q)", binaryProtocol, stdioProtocolsEnv, protocols)
	}
	body, err := json.Marshal(manifest.descriptor(maxInFlight()))
	if err != nil {
		return fmt.Errorf("encode descriptor: %w", err)
	}
	return writeFrame(out, reply{Status: statusOK, Payload: body}.appendJSON(nil))
}

// serveCalls answers call frames until the end of input, each on a goroutine
// of its own: the host keeps at most maxInFlight of them unanswered. It
// returns once every started call has been answered.
func serveCalls(in io.Reader, out io.Writer, srv *server) error {
	resp := &responder{w: out}
	var wg sync.WaitGroup
	for {
		frame, err := readCall(in)
		if err != nil {
			wg.Wait()
			if failure := resp.failure(); failure != nil {
				return failure
			}
			return err
		}
		wg.Add(1)
		go func() {
			defer wg.Done()
			resp.respond(frame.id, handleCall(srv, frame.contract, frame.method, frame.payload))
		}()
	}
}

// handleCall answers one call. A panic in the parser or printer becomes a
// failed reply, so one bad file does not stop the process and fail every
// later file.
func handleCall(srv *server, contract, method string, payload []byte) (answer reply) {
	defer func() {
		if r := recover(); r != nil {
			fmt.Fprintf(os.Stderr, "poly-sh: panic: %v\n%s", r, debug.Stack())
			answer = failed("internal error: %v", r)
		}
	}()
	if contract != contractPoly {
		return failed("contract %q is not provided", contract)
	}
	switch method {
	case methodDescr:
		out, err := srv.describe()
		if err != nil {
			return failed("%v", err)
		}
		return reply{Status: statusOK, Payload: out}
	case methodCall:
		header, body, err := unpackCall(payload)
		if err != nil {
			return failed("%v", err)
		}
		out, err := srv.call(header, body)
		if err != nil {
			return failed("%v", err)
		}
		return reply{Status: statusOK, Payload: out}
	default:
		return failed("method %q is not served", method)
	}
}

func failed(format string, args ...any) reply {
	return reply{Status: statusFailed, Payload: []byte(fmt.Sprintf(format, args...))}
}

// unpackCall splits a `call` payload: a little-endian u32 header length, the
// MessagePack request header, then the file content (the rest).
func unpackCall(payload []byte) (header, body []byte, err error) {
	if len(payload) < 4 {
		return nil, nil, errors.New("call payload is shorter than its 4-byte header length")
	}
	n := uint64(binary.LittleEndian.Uint32(payload))
	rest := payload[4:]
	if n > uint64(len(rest)) {
		return nil, nil, fmt.Errorf("call header length %d exceeds the %d bytes that follow", n, len(rest))
	}
	return rest[:n], rest[n:], nil
}
