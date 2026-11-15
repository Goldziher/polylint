package main

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"io"
	"math/rand/v2"
	"testing"
)

func TestReadFrame(t *testing.T) {
	withLen := func(n uint32, body string) []byte {
		out := make([]byte, 4)
		binary.LittleEndian.PutUint32(out, n)
		return append(out, body...)
	}
	tests := []struct {
		name    string
		in      []byte
		want    string
		wantErr bool
	}{
		{"one frame", withLen(2, "{}"), "{}", false},
		{"zero length", withLen(0, ""), "", true},
		{"over the limit", withLen(maxFrame+1, ""), "", true},
		{"truncated body", withLen(5, "ab"), "", true},
		{"eof", nil, "", true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got, err := readFrame(bytes.NewReader(tt.in))
			if (err != nil) != tt.wantErr {
				t.Fatalf("err = %v, wantErr %v", err, tt.wantErr)
			}
			if string(got) != tt.want {
				t.Errorf("got %q, want %q", got, tt.want)
			}
		})
	}
}

func TestWriteFrameRoundTrips(t *testing.T) {
	var buf bytes.Buffer
	if err := writeFrame(&buf, []byte(`{"status":0}`)); err != nil {
		t.Fatal(err)
	}
	got, err := readFrame(&buf)
	if err != nil || string(got) != `{"status":0}` {
		t.Fatalf("got %q, %v", got, err)
	}
	if err := writeFrame(io.Discard, nil); err == nil {
		t.Fatal("want an error for an empty frame")
	}
}

func randomBytes(n int) []byte {
	r := rand.New(rand.NewPCG(7, 11)) //nolint:gosec // deterministic test data, not a secret
	out := make([]byte, n)
	for i := range out {
		out[i] = byte(r.UintN(256)) //nolint:gosec // UintN(256) fits a byte
	}
	return out
}

func allByteValues() []byte {
	out := make([]byte, 256)
	for i := range out {
		out[i] = byte(i)
	}
	return out
}

func TestByteArrayRoundTrips(t *testing.T) {
	tests := []struct {
		name string
		in   []byte
	}{
		{"empty", []byte{}},
		{"one byte", []byte{0}},
		{"every value", allByteValues()},
		{"random", randomBytes(100_000)},
		{"three mebibytes", randomBytes(3 * 1024 * 1024)},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			encoded, err := byteArray(tt.in).MarshalJSON()
			if err != nil {
				t.Fatal(err)
			}
			var got byteArray
			if err := got.UnmarshalJSON(encoded); err != nil {
				t.Fatal(err)
			}
			if !bytes.Equal(got, tt.in) {
				t.Fatalf("round trip changed %d bytes into %d", len(tt.in), len(got))
			}
		})
	}
}

// The wire format is the one encoding/json writes for a []int: the encoder and
// decoder must stay byte-compatible with it, whatever the host's serializer emits.
func TestByteArrayMatchesAnIntSliceOnTheWire(t *testing.T) {
	for _, in := range [][]byte{{}, {0}, {9, 10, 99, 100, 255}, allByteValues(), randomBytes(5000)} {
		numbers := make([]int, len(in))
		for i, v := range in {
			numbers[i] = int(v)
		}
		want, err := json.Marshal(numbers)
		if err != nil {
			t.Fatal(err)
		}
		got, err := byteArray(in).MarshalJSON()
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(got, want) {
			t.Fatalf("encoded %v as %s, want %s", in, got, want)
		}
		var decoded byteArray
		if err := json.Unmarshal(want, &decoded); err != nil || !bytes.Equal(decoded, in) {
			t.Fatalf("decoded %s as %v, %v", want, decoded, err)
		}
	}
}

func TestByteArrayDecoding(t *testing.T) {
	tests := []struct {
		name    string
		in      string
		want    []byte
		wantErr bool
	}{
		{"spaced", " [ 1 , 2,\n3\t] ", []byte{1, 2, 3}, false},
		{"empty with space", "[ ]", []byte{}, false},
		{"null is empty", "null", []byte{}, false},
		{"over 255", "[256]", nil, true},
		{"far over 255", "[99999999999999999999]", nil, true},
		{"negative", "[-1]", nil, true},
		{"fraction", "[1.5]", nil, true},
		{"exponent", "[1e2]", nil, true},
		{"string element", `["a"]`, nil, true},
		{"not an array", "7", nil, true},
		{"unterminated", "[1,2", nil, true},
		{"trailing comma", "[1,]", nil, true},
		{"missing comma", "[1 2]", nil, true},
		{"trailing data", "[1] 2", nil, true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			var got byteArray
			err := got.UnmarshalJSON([]byte(tt.in))
			if (err != nil) != tt.wantErr {
				t.Fatalf("err = %v, wantErr %v", err, tt.wantErr)
			}
			if !tt.wantErr && !bytes.Equal(got, tt.want) {
				t.Fatalf("got %v, want %v", got, tt.want)
			}
		})
	}
}

func TestReplyAppendJSONMatchesEncodingJSON(t *testing.T) {
	for _, r := range []reply{{Status: 0, Payload: []byte{}}, {Status: 3, Payload: []byte("boom")}, {Status: 0, Payload: allByteValues()}} {
		want, err := json.Marshal(r)
		if err != nil {
			t.Fatal(err)
		}
		if got := r.appendJSON(nil); !bytes.Equal(got, want) {
			t.Fatalf("appendJSON = %s, want %s", got, want)
		}
	}
}
