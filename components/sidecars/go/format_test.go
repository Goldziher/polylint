package main

import (
	"os"
	"path/filepath"
	"testing"
)

const (
	testLang   = "1.22"
	testModule = "example.com/mod"
)

func readTestdata(t *testing.T, elem ...string) []byte {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(append([]string{"testdata"}, elem...)...)) //nolint:gosec // fixed fixture names
	if err != nil {
		t.Fatal(err)
	}
	return data
}

// The golden files are the output of `gofumpt` v0.12.0 on each fixture, run as
// `gofumpt [-extra] -lang=go1.22 -modpath=example.com/mod`, so these tests pin
// byte parity with the tool whose rules the component applies. Import grouping
// is off here because gofumpt does not group imports: it must not change what
// gofumpt wrote.
func TestFormatMatchesGofumptGoldens(t *testing.T) {
	for _, fixture := range []string{"basic", "already"} {
		for _, extra := range []bool{false, true} {
			name := "plain"
			if extra {
				name = "extra"
			}
			t.Run(fixture+"/"+name, func(t *testing.T) {
				src := readTestdata(t, fixture+".go")
				want := string(readTestdata(t, "golden", fixture+"."+name+".go.golden"))
				opts := formatOptions{gofumpt: true, extra: extra, langVersion: testLang, modulePath: testModule}
				got, changed, err := formatSource(fixture+".go", src, opts)
				if err != nil {
					t.Fatalf("formatSource: %v", err)
				}
				if !changed {
					got = string(src)
				}
				if got != want {
					t.Errorf("output differs from gofumpt\n got: %q\nwant: %q", got, want)
				}
				if changed == (string(src) == want) {
					t.Errorf("changed = %v, but source equals golden is %v", changed, string(src) == want)
				}
			})
		}
	}
}

// Grouping on top of gofumpt must leave gofumpt's own output alone when the
// imports are already grouped.
func TestGroupingKeepsAGroupedFileAsGofumptWroteIt(t *testing.T) {
	src := readTestdata(t, "basic.go")
	want := string(readTestdata(t, "golden", "basic.extra.go.golden"))
	opts := formatOptions{gofumpt: true, extra: true, groupImports: true, langVersion: testLang, modulePath: testModule}
	got, _, err := formatSource("basic.go", src, opts)
	if err != nil {
		t.Fatal(err)
	}
	if got != want {
		t.Errorf("grouping changed gofumpt's output\n got: %q\nwant: %q", got, want)
	}
}

func TestGroupImports(t *testing.T) {
	src := readTestdata(t, "grouping.go")
	tests := []struct {
		name  string
		local string
		want  string
	}{
		{
			name: "standard library first, then third party",
			want: "package grouping\n\nimport (\n\t\"fmt\"\n\t\"os\"\n\n\t\"example.com/mod/inner\"\n\t\"github.com/a/b\"\n)\n",
		},
		{
			name:  "local prefixes form their own last group",
			local: "example.com/mod",
			want:  "package grouping\n\nimport (\n\t\"fmt\"\n\t\"os\"\n\n\t\"github.com/a/b\"\n\n\t\"example.com/mod/inner\"\n)\n",
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			opts := formatOptions{gofumpt: true, groupImports: true, localPrefixes: tt.local, langVersion: testLang}
			got, changed, err := formatSource("grouping.go", src, opts)
			if err != nil {
				t.Fatal(err)
			}
			if !changed {
				t.Fatal("want the imports regrouped")
			}
			if len(got) < len(tt.want) || got[:len(tt.want)] != tt.want {
				t.Errorf("imports\n got: %q\nwant prefix: %q", got, tt.want)
			}
		})
	}
}

func TestGroupingOffAndGofumptOffIsPlainGofmt(t *testing.T) {
	src := []byte("package p\n\nfunc  f( ) {\n\n\tx := 1\n\t_ = x\n\n}\n")
	got, changed, err := formatSource("p.go", src, formatOptions{})
	if err != nil || !changed {
		t.Fatalf("changed = %v, err = %v", changed, err)
	}
	// gofmt keeps the blank lines gofumpt would remove.
	if want := "package p\n\nfunc f() {\n\n\tx := 1\n\t_ = x\n\n}\n"; got != want {
		t.Errorf("got %q, want %q", got, want)
	}
}

func TestUnusedImportsAreNotRemoved(t *testing.T) {
	src := []byte("package p\n\nimport (\n\t\"os\"\n\t\"fmt\"\n)\n")
	got, _, err := formatSource("p.go", src, formatOptions{gofumpt: true, groupImports: true})
	if err != nil {
		t.Fatal(err)
	}
	if want := "package p\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n"; got != want {
		t.Errorf("got %q, want %q: formatting must sort imports and never drop or add one", got, want)
	}
}

func TestFormatRejectsUnparsableSource(t *testing.T) {
	src := readTestdata(t, "broken.go")
	for _, opts := range []formatOptions{{gofumpt: true}, {}, {groupImports: true}} {
		if _, _, err := formatSource("broken.go", src, opts); err == nil {
			t.Errorf("%+v: want a parse error", opts)
		}
	}
}

func TestGoLang(t *testing.T) {
	tests := []struct {
		in      string
		want    string
		wantErr bool
	}{
		{"", "", false},
		{"1.22", "go1.22", false},
		{"1.22.1", "go1.22.1", false},
		{"go1.21", "go1.21", false},
		{"1.22rc1", "go1.22rc1", false},
		{"nonsense", "", true},
		{"v1.22", "", true},
	}
	for _, tt := range tests {
		t.Run(tt.in, func(t *testing.T) {
			got, err := goLang(tt.in)
			if (err != nil) != tt.wantErr || got != tt.want {
				t.Errorf("goLang(%q) = %q, %v; want %q, error %v", tt.in, got, err, tt.want, tt.wantErr)
			}
		})
	}
}

func TestAnInvalidLangVersionIsAnErrorNotAPanic(t *testing.T) {
	if _, _, err := formatSource("p.go", []byte("package p\n"), formatOptions{gofumpt: true, langVersion: "bogus"}); err == nil {
		t.Fatal("want an error")
	}
}

// The language version gates rules: octal literals are only rewritten from Go
// 1.13, so the same bytes format differently under different go.mod versions.
func TestLangVersionGatesRules(t *testing.T) {
	src := []byte("package p\n\nvar x = 0777\n")
	old, _, err := formatSource("p.go", src, formatOptions{gofumpt: true, langVersion: "1.12"})
	if err != nil {
		t.Fatal(err)
	}
	if old != "" {
		t.Errorf("go1.12 output %q, want the source unchanged", old)
	}
	got, changed, err := formatSource("p.go", src, formatOptions{gofumpt: true, langVersion: "1.13"})
	if err != nil || !changed || got != "package p\n\nvar x = 0o777\n" {
		t.Errorf("go1.13 = %q, changed %v, err %v", got, changed, err)
	}
}

func TestFormatNeedsNoGoToolchain(t *testing.T) {
	t.Setenv("PATH", "/nonexistent")
	t.Setenv("GOFLAGS", "")
	t.Setenv("HOME", t.TempDir())
	src := readTestdata(t, "grouping.go")
	if _, changed, err := formatSource("grouping.go", src, formatOptions{gofumpt: true, groupImports: true}); err != nil || !changed {
		t.Fatalf("changed = %v, err = %v: formatting must not shell out to go", changed, err)
	}
}
