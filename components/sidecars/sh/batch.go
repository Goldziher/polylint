package main

import (
	"runtime"
	"sync"
	"sync/atomic"

	"github.com/vmihailenco/msgpack/v5"
)

// handleBatchSupport answers Request::BatchSupport: this component formats a
// batch in one call and lints nothing.
func (s *server) handleBatchSupport(fields msgpack.RawMessage) (any, *wireError) {
	var req batchSupportRequest
	if err := msgpack.Unmarshal(fields, &req); err != nil {
		return nil, newWireError(errKindProtocol, "malformed BatchSupport: %v", err)
	}
	if _, failure := s.lookup(req.Engine, req.Config); failure != nil {
		return nil, failure
	}
	return map[string]any{"BatchSupport": map[string]any{"format": true, "lint": false}}, nil
}

// lookup checks that an engine is the one hosted here and its config is
// registered.
func (s *server) lookup(engine string, config uint32) (engineConfig, *wireError) {
	if engine != engineName {
		return engineConfig{}, newWireError(errKindUnknownEng, "engine %q is not hosted here", engine)
	}
	cfg, ok := s.configs[config]
	if !ok {
		return engineConfig{}, newWireError(errKindUnknownCfg, "config %d is not registered", config)
	}
	return cfg, nil
}

// handleFormatBatch answers Request::FormatBatch: one result per member, in
// order. A member that cannot be formatted is its own error and does not fail
// its siblings; a member window outside the body is a malformed request.
func (s *server) handleFormatBatch(fields msgpack.RawMessage, body []byte) (any, []wireLog, *wireError) {
	var req formatBatchRequest
	if err := msgpack.Unmarshal(fields, &req); err != nil {
		return nil, nil, newWireError(errKindProtocol, "malformed FormatBatch: %v", err)
	}
	cfg, failure := s.lookup(req.Engine, req.Config)
	if failure != nil {
		return nil, nil, failure
	}
	size := uint64(len(body))
	for _, file := range req.Files {
		if file.Offset > size || file.Len > size-file.Offset {
			return nil, nil, newWireError(errKindProtocol, "%s: slice %d+%d is outside the %d-byte body", file.Path, file.Offset, file.Len, size)
		}
	}
	results := s.formatMembers(cfg, req.Files, body)
	items := make([]any, len(results))
	var logs []wireLog
	for i, result := range results {
		items[i] = result.item
		logs = append(logs, result.logs...)
	}
	return map[string]any{"FormatBatch": items}, logs, nil
}

// batchResult is one member's WireFormatItem and the logs it produced.
type batchResult struct {
	item map[string]any
	logs []wireLog
}

// formatMembers formats the members on a pool of goroutines sized to the CPUs
// and returns the results in member order. The host serializes calls to this
// process, so this is the only parallelism a sidecar has.
func (s *server) formatMembers(cfg engineConfig, files []wireFileSlice, body []byte) []batchResult {
	results := make([]batchResult, len(files))
	var next atomic.Int64
	var wg sync.WaitGroup
	for range min(runtime.GOMAXPROCS(0), len(files)) {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for {
				i := int(next.Add(1)) - 1
				if i >= len(files) {
					return
				}
				results[i] = s.formatMember(cfg, files[i], body)
			}
		}()
	}
	wg.Wait()
	return results
}

// formatMember formats one member. A panic in the parser or printer is that
// member's error: it runs on a goroutine of its own, where nothing else would
// recover it.
func (s *server) formatMember(cfg engineConfig, file wireFileSlice, body []byte) (result batchResult) {
	defer func() {
		if r := recover(); r != nil {
			failure := newWireError(errKindError, "internal error formatting %s: %v", file.Path, r)
			result = batchResult{item: map[string]any{"Err": failure}}
		}
	}()
	end := file.Offset + file.Len
	output, logs, failure := s.formatContent(cfg, file.Path, body[file.Offset:end:end])
	if failure != nil {
		return batchResult{item: map[string]any{"Err": failure}}
	}
	return batchResult{item: map[string]any{"Ok": output}, logs: logs}
}
