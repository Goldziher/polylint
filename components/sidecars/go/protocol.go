package main

import (
	"bytes"
	"errors"
	"fmt"

	"github.com/vmihailenco/msgpack/v5"
)

// protocolVersion is poly's component protocol version
// (poly_component_protocol::PROTOCOL_VERSION).
//
// Version 3 encodes requests and responses positionally: a struct is a
// MessagePack array of its fields in declaration order, every field present.
// Enums stay externally tagged by variant name. The decoder reads an array
// into a Go struct field by field, so every request struct below declares
// every field of its Rust counterpart, in order. `describe` keeps named fields.
const protocolVersion = 3

// Wire error kinds (WireErrorKind).
const (
	errKindError       = "Error"
	errKindUnknownCfg  = "UnknownConfig"
	errKindUnknownEng  = "UnknownEngine"
	errKindProtocol    = "Protocol"
	errKindUnsupported = "Unsupported"
)

// Wire log levels (WireLogLevel).
const logDebug = "Debug"

// wireError mirrors poly_component_protocol::WireError.
type wireError struct {
	Kind    string   `msgpack:"kind"`
	Message string   `msgpack:"message"`
	Chain   []string `msgpack:"chain"`
}

func (e *wireError) Error() string { return e.Kind + ": " + e.Message }

func newWireError(kind, format string, args ...any) *wireError {
	return &wireError{Kind: kind, Message: fmt.Sprintf(format, args...), Chain: []string{}}
}

// wireLog mirrors poly_component_protocol::WireLog.
type wireLog struct {
	Level   string `msgpack:"level"`
	Target  string `msgpack:"target"`
	Message string `msgpack:"message"`
}

// wireFile mirrors poly_component_protocol::WireFile.
type wireFile struct {
	Path     string `msgpack:"path"`
	Language string `msgpack:"language"`
	Len      uint64 `msgpack:"len"`
}

// wireFileSlice mirrors poly_component_protocol::WireFileSlice: one batch
// member, a window of the request body.
type wireFileSlice struct {
	Path     string `msgpack:"path"`
	Language string `msgpack:"language"`
	Offset   uint64 `msgpack:"offset"`
	Len      uint64 `msgpack:"len"`
}

// request is a decoded request header: the variant name and its fields.
type request struct {
	variant string
	fields  msgpack.RawMessage
}

// decodeRequest splits an externally tagged request header
// (`{"Variant": [fields]}`) into its variant and raw fields.
func decodeRequest(header []byte) (request, error) {
	var tagged map[string]msgpack.RawMessage
	if err := msgpack.Unmarshal(header, &tagged); err != nil {
		return request{}, fmt.Errorf("decode request header: %w", err)
	}
	if len(tagged) != 1 {
		return request{}, fmt.Errorf("request header has %d variants, want 1", len(tagged))
	}
	for variant, fields := range tagged {
		return request{variant: variant, fields: fields}, nil
	}
	return request{}, errors.New("unreachable")
}

// initRequest mirrors Request::Init.
type initRequest struct {
	Protocol    uint32 `msgpack:"protocol"`
	HostVersion string `msgpack:"host_version"`
	Threads     uint32 `msgpack:"threads"`
	StackSize   uint64 `msgpack:"stack_size"`
	MaxLogLevel string `msgpack:"max_log_level"`
}

// registerConfigRequest mirrors Request::RegisterConfig; Config is a WireValue
// table.
type registerConfigRequest struct {
	ID     uint32 `msgpack:"id"`
	Config any    `msgpack:"config"`
}

// formatRequest mirrors Request::Format.
type formatRequest struct {
	Engine string   `msgpack:"engine"`
	Config uint32   `msgpack:"config"`
	File   wireFile `msgpack:"file"`
}

// formatBatchRequest mirrors Request::FormatBatch.
type formatBatchRequest struct {
	Engine string          `msgpack:"engine"`
	Config uint32          `msgpack:"config"`
	Files  []wireFileSlice `msgpack:"files"`
}

// batchSupportRequest mirrors Request::BatchSupport.
type batchSupportRequest struct {
	Engine string `msgpack:"engine"`
	Config uint32 `msgpack:"config"`
}

// engineConfig is the part of a registered engine config this component reads:
// the `options` table. `lang_version` and `module_path` are filled in by the
// host from the file's go.mod (host hook `go-mod`), the rest are the user's.
type engineConfig struct {
	formatOptions
}

// defaultConfig is what an engine with no options does: gofumpt rules and
// goimports grouping, the output `golangci-lint fmt` gives.
func defaultConfig() engineConfig {
	return engineConfig{formatOptions{gofumpt: true, groupImports: true}}
}

// parseEngineConfig reads the `options` table of a WireValue engine config. An
// unknown key is ignored: the host has already checked the declared keys.
func parseEngineConfig(value any) (engineConfig, error) {
	cfg := defaultConfig()
	table, err := wireTable(value)
	if err != nil {
		return cfg, fmt.Errorf("engine config: %w", err)
	}
	options, ok := table["options"]
	if !ok {
		return cfg, nil
	}
	opts, err := wireTable(options)
	if err != nil {
		return cfg, fmt.Errorf("engine config options: %w", err)
	}
	for key, dst := range map[string]*bool{
		optionsGofumpt: &cfg.gofumpt, optionsExtra: &cfg.extra, optionsGroupImports: &cfg.groupImports,
	} {
		if raw, ok := opts[key]; ok {
			if *dst, err = wireBoolean(raw); err != nil {
				return cfg, fmt.Errorf("engine config %s: %w", key, err)
			}
		}
	}
	for key, dst := range map[string]*string{
		optionsLangVersion: &cfg.langVersion, optionsModulePath: &cfg.modulePath, optionsLocalPrefixes: &cfg.localPrefixes,
	} {
		if raw, ok := opts[key]; ok {
			if *dst, err = wireString(raw); err != nil {
				return cfg, fmt.Errorf("engine config %s: %w", key, err)
			}
		}
	}
	return cfg, nil
}

// wireTable unwraps a WireValue::Table (`{"Table": [[key, value], ...]}`) into
// a map of still-wrapped values.
func wireTable(value any) (map[string]any, error) {
	inner, err := wireVariant(value, "Table")
	if err != nil {
		return nil, err
	}
	pairs, ok := inner.([]any)
	if !ok {
		return nil, fmt.Errorf("table body is %T, want an array of pairs", inner)
	}
	table := make(map[string]any, len(pairs))
	for _, pair := range pairs {
		kv, ok := pair.([]any)
		if !ok || len(kv) != 2 {
			return nil, fmt.Errorf("table entry is %v, want a [key, value] pair", pair)
		}
		key, ok := kv[0].(string)
		if !ok {
			return nil, fmt.Errorf("table key is %T, want a string", kv[0])
		}
		table[key] = kv[1]
	}
	return table, nil
}

func wireString(value any) (string, error) {
	inner, err := wireVariant(value, "String")
	if err != nil {
		return "", err
	}
	str, ok := inner.(string)
	if !ok {
		return "", fmt.Errorf("string body is %T", inner)
	}
	return str, nil
}

func wireBoolean(value any) (bool, error) {
	inner, err := wireVariant(value, "Boolean")
	if err != nil {
		return false, err
	}
	b, ok := inner.(bool)
	if !ok {
		return false, fmt.Errorf("boolean body is %T", inner)
	}
	return b, nil
}

// wireVariant unwraps a single-entry externally tagged map and checks its tag.
func wireVariant(value any, tag string) (any, error) {
	m, ok := value.(map[string]any)
	if !ok || len(m) != 1 {
		return nil, fmt.Errorf("want a {%q: ...} value, got %v", tag, value)
	}
	inner, ok := m[tag]
	if !ok {
		return nil, fmt.Errorf("want variant %q, got %v", tag, value)
	}
	return inner, nil
}

// encodeResponse encodes `Response { result, logs }` positionally, as
// `[result, logs]` with every struct inside an array: `ok` is the payload of a
// success, or failure the error.
func encodeResponse(ok any, failure *wireError, logs []wireLog) ([]byte, error) {
	var result map[string]any
	if failure != nil {
		result = map[string]any{"Err": failure}
	} else {
		result = map[string]any{"Ok": ok}
	}
	if logs == nil {
		logs = []wireLog{}
	}
	var buf bytes.Buffer
	enc := msgpack.NewEncoder(&buf)
	enc.SetCustomStructTag("msgpack")
	enc.UseArrayEncodedStructs(true)
	if err := enc.Encode([]any{result, logs}); err != nil {
		return nil, fmt.Errorf("encode response: %w", err)
	}
	return buf.Bytes(), nil
}
