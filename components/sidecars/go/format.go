package main

import (
	"bytes"
	"fmt"
	gofmt "go/format"
	goversion "go/version"
	"sync"

	"golang.org/x/tools/imports"
	"mvdan.cc/gofumpt/format"
)

// formatOptions are the settings poly passes to its Go formatter.
type formatOptions struct {
	// gofumpt applies gofumpt's rules; false leaves plain gofmt output.
	gofumpt bool
	// extra enables gofumpt's extra rules (`gofumpt -extra`).
	extra bool
	// langVersion and modulePath are the module's `go` directive (`1.22`) and
	// path, which gofumpt needs to decide which rules apply.
	langVersion string
	modulePath  string
	// groupImports orders each import block by group, as goimports does.
	groupImports bool
	// localPrefixes is goimports' `-local`: comma-separated import path
	// prefixes placed in a group after third-party imports.
	localPrefixes string
}

var localPrefixMu sync.Mutex

// setLocalPrefix sets goimports' `-local`, which the library keeps in a package
// variable. It writes only on a change, so concurrent formatters that share a
// config (a batch) only ever read it.
func setLocalPrefix(prefix string) {
	localPrefixMu.Lock()
	defer localPrefixMu.Unlock()
	if imports.LocalPrefix != prefix {
		imports.LocalPrefix = prefix
	}
}

// goLang is the form go/version and gofumpt accept for a go.mod `go` line:
// `go1.22` or `go1.22.1`. A bare `1.22` gets the prefix, and an invalid
// version is an error rather than a panic inside gofumpt.
func goLang(version string) (string, error) {
	if version == "" {
		return "", nil
	}
	lang := version
	if len(lang) < 2 || lang[:2] != "go" {
		lang = "go" + lang
	}
	if !goversion.IsValid(lang) {
		return "", fmt.Errorf("invalid Go version %q", version)
	}
	return lang, nil
}

// formatSource formats Go source: gofumpt (a superset of gofmt) and goimports'
// import grouping, never adding or removing an import, which needs the `go`
// command. It returns changed=false when the source is already formatted. A
// source that does not parse is an error, which the caller reports as "leave
// the file alone".
func formatSource(path string, src []byte, opts formatOptions) (formatted string, changed bool, err error) {
	lang, err := goLang(opts.langVersion)
	if err != nil {
		return "", false, err
	}
	out := src
	if opts.gofumpt {
		fumpt := format.Options{LangVersion: lang, ModulePath: opts.modulePath}
		if opts.extra {
			if err := fumpt.Extra.Set("true"); err != nil {
				return "", false, fmt.Errorf("enable gofumpt extra rules: %w", err)
			}
		}
		out, err = format.Source(out, fumpt)
	} else {
		out, err = gofmt.Source(out)
	}
	if err != nil {
		return "", false, fmt.Errorf("format %s: %w", path, err)
	}
	if opts.groupImports {
		setLocalPrefix(opts.localPrefixes)
		if out, err = imports.Process(path, out, &imports.Options{
			Comments: true, TabIndent: true, TabWidth: 8, FormatOnly: true,
		}); err != nil {
			return "", false, fmt.Errorf("goimports %s: %w", path, err)
		}
	}
	if bytes.Equal(out, src) {
		return "", false, nil
	}
	return string(out), true, nil
}
