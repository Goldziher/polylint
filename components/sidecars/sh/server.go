package main

import (
	"fmt"
	"sync"
	"unicode/utf8"

	"github.com/vmihailenco/msgpack/v5"
)

// server answers poly's component protocol. Calls arrive concurrently (stdio
// protocol 2 pipelines them), so the session state is guarded by mu.
type server struct {
	componentVersion string
	format           formatFunc

	mu          sync.RWMutex
	initialized bool
	configs     map[uint32]engineConfig
}

// formatFunc is formatSource's shape; tests substitute it.
type formatFunc func(path string, src []byte, opts formatOptions) (formatted string, changed bool, err error)

func newServer(componentVersion string) *server {
	return &server{componentVersion: componentVersion, configs: make(map[uint32]engineConfig), format: formatSource}
}

// config is the config registered under id.
func (s *server) config(id uint32) (engineConfig, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	cfg, ok := s.configs[id]
	return cfg, ok
}

// describe answers the `describe` method.
func (s *server) describe() ([]byte, error) {
	desc, err := describe(s.componentVersion)
	if err != nil {
		return nil, err
	}
	out, err := msgpack.Marshal(desc)
	if err != nil {
		return nil, fmt.Errorf("encode describe: %w", err)
	}
	return out, nil
}

// call answers the `call` method: one request header and the file content.
// A failure the protocol can express is a Response with an error result, not a
// Go error; only an undecodable header is a Go error.
func (s *server) call(header, body []byte) ([]byte, error) {
	req, err := decodeRequest(header)
	if err != nil {
		return nil, err
	}
	var (
		payload any
		logs    []wireLog
		failure *wireError
	)
	switch req.variant {
	case "Init":
		payload, failure = s.handleInit(req.fields)
	case "RegisterConfig":
		payload, failure = s.handleRegisterConfig(req.fields)
	case "Format":
		payload, logs, failure = s.handleFormat(req.fields, body)
	case "FormatBatch":
		payload, logs, failure = s.handleFormatBatch(req.fields, body)
	case "BatchSupport":
		payload, failure = s.handleBatchSupport(req.fields)
	default:
		failure = newWireError(errKindUnsupported, "the sh component does not serve %s", req.variant)
	}
	return encodeResponse(payload, failure, logs)
}

func (s *server) handleInit(fields msgpack.RawMessage) (any, *wireError) {
	var init initRequest
	if err := msgpack.Unmarshal(fields, &init); err != nil {
		return nil, newWireError(errKindProtocol, "malformed Init: %v", err)
	}
	if init.Protocol != protocolVersion {
		return nil, newWireError(errKindProtocol, "host speaks protocol %d, this component speaks %d", init.Protocol, protocolVersion)
	}
	s.mu.Lock()
	s.initialized = true
	s.mu.Unlock()
	return map[string]any{"Init": []any{uint32(protocolVersion), s.componentVersion}}, nil
}

func (s *server) handleRegisterConfig(fields msgpack.RawMessage) (any, *wireError) {
	var reg registerConfigRequest
	if err := msgpack.Unmarshal(fields, &reg); err != nil {
		return nil, newWireError(errKindProtocol, "malformed RegisterConfig: %v", err)
	}
	cfg, err := parseEngineConfig(reg.Config)
	if err != nil {
		return nil, newWireError(errKindProtocol, "%v", err)
	}
	s.mu.Lock()
	s.configs[reg.ID] = cfg
	s.mu.Unlock()
	return "Ok", nil
}

func (s *server) handleFormat(fields msgpack.RawMessage, body []byte) (any, []wireLog, *wireError) {
	var req formatRequest
	if err := msgpack.Unmarshal(fields, &req); err != nil {
		return nil, nil, newWireError(errKindProtocol, "malformed Format: %v", err)
	}
	if req.Engine != engineName {
		return nil, nil, newWireError(errKindUnknownEng, "engine %q is not hosted here", req.Engine)
	}
	cfg, ok := s.config(req.Config)
	if !ok {
		return nil, nil, newWireError(errKindUnknownCfg, "config %d is not registered", req.Config)
	}
	if req.File.Len != uint64(len(body)) {
		return nil, nil, newWireError(errKindProtocol, "file.len is %d but the body has %d bytes", req.File.Len, len(body))
	}
	output, logs, failure := s.formatContent(cfg, req.File.Path, body)
	if failure != nil {
		return nil, nil, failure
	}
	return map[string]any{"Formatted": output}, logs, nil
}

// formatContent formats one file's bytes: the WireFormatOutput value, the debug
// log of a file left alone, or the failure of a file that cannot be formatted.
// It is safe to call from several goroutines.
func (s *server) formatContent(cfg engineConfig, path string, content []byte) (any, []wireLog, *wireError) {
	if !utf8.Valid(content) {
		return nil, nil, newWireError(errKindError, "%s is not valid UTF-8", path)
	}
	indent := cfg.indentWidth
	if cfg.useTabs {
		indent = 0
	}
	formatted, changed, err := s.format(path, content, formatOptions{indent: indent})
	if err != nil {
		// Like the host tool's non-zero exit: a file that does not parse is
		// left alone, with the reason at debug level.
		log := wireLog{Level: logDebug, Target: "poly_sh", Message: err.Error()}
		return "Unchanged", []wireLog{log}, nil
	}
	if !changed {
		return "Unchanged", nil, nil
	}
	return map[string]any{"Formatted": formatted}, nil, nil
}
