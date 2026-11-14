package main

import (
	"fmt"
	"runtime/debug"
)

const (
	// engineName is the built-in engine this component replaces, so
	// `[engines.gofmt]` keeps working.
	engineName     = "gofmt"
	languageGo     = "go"
	gofumptModule  = "mvdan.cc/gofumpt"
	toolsModule    = "golang.org/x/tools"
	optionsEnabled = "enabled"

	optionsGofumpt       = "gofumpt"
	optionsExtra         = "extra"
	optionsGroupImports  = "group_imports"
	optionsLocalPrefixes = "local_prefixes"
	optionsLangVersion   = "lang_version"
	optionsModulePath    = "module_path"

	// The type bits of WireOptionKey.types.
	optionTypeBoolean = 1
	optionTypeString  = 8
)

type wireCapabilities struct {
	Lint    bool `msgpack:"lint"`
	Format  bool `msgpack:"format"`
	Fix     bool `msgpack:"fix"`
	Project bool `msgpack:"project"`
}

type wireOptionKey struct {
	Name  string `msgpack:"name"`
	Types uint8  `msgpack:"types"`
}

type wireOptionKeys struct {
	Table         string          `msgpack:"table"`
	Checked       bool            `msgpack:"checked"`
	Declared      []wireOptionKey `msgpack:"declared"`
	RuleSelection bool            `msgpack:"rule_selection"`
	HasDerived    bool            `msgpack:"has_derived"`
	Note          *string         `msgpack:"note"`
}

type wireEngineDesc struct {
	Name                       string           `msgpack:"name"`
	Languages                  []string         `msgpack:"languages"`
	Capabilities               wireCapabilities `msgpack:"capabilities"`
	Version                    string           `msgpack:"version"`
	OptionKeys                 []wireOptionKeys `msgpack:"option_keys"`
	Hooks                      uint32           `msgpack:"hooks"`
	TakesOverTierOne           bool             `msgpack:"takes_over_tier_one"`
	ClaimedPaths               []string         `msgpack:"claimed_paths"`
	HostHook                   string           `msgpack:"host_hook"`
	SupplementsTierOne         bool             `msgpack:"supplements_tier_one"`
	SelfManagesEnabled         bool             `msgpack:"self_manages_enabled"`
	SupersedesGenericFormatter bool             `msgpack:"supersedes_generic_formatter"`
	BatchShardFloor            uint32           `msgpack:"batch_shard_floor"`
}

type wireDescribe struct {
	Protocol         uint32           `msgpack:"protocol"`
	ComponentVersion string           `msgpack:"component_version"`
	Engines          []wireEngineDesc `msgpack:"engines"`
}

// toolVersions is "gofumpt <v> goimports <v>": the versions of the two modules
// this binary was built with. It is the engine version, so an upgrade of either
// invalidates the host's format cache.
func toolVersions() (string, error) {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		return "", fmt.Errorf("no build info in this binary")
	}
	versions := map[string]string{}
	for _, dep := range info.Deps {
		versions[dep.Path] = dep.Version
	}
	for _, module := range []string{gofumptModule, toolsModule} {
		if versions[module] == "" {
			return "", fmt.Errorf("build info does not list %s", module)
		}
	}
	return fmt.Sprintf("gofumpt-%s+x-tools-%s", versions[gofumptModule], versions[toolsModule]), nil
}

// describe is poly's `describe()`: the one engine this component hosts. It
// must equal the component index entry the host resolved the component from.
func describe(componentVersion string) (wireDescribe, error) {
	version, err := toolVersions()
	if err != nil {
		return wireDescribe{}, err
	}
	boolean := func(name string) wireOptionKey { return wireOptionKey{Name: name, Types: optionTypeBoolean} }
	text := func(name string) wireOptionKey { return wireOptionKey{Name: name, Types: optionTypeString} }
	return wireDescribe{
		Protocol:         protocolVersion,
		ComponentVersion: componentVersion,
		Engines: []wireEngineDesc{{
			Name:         engineName,
			Languages:    []string{languageGo},
			Capabilities: wireCapabilities{Format: true},
			Version:      version,
			OptionKeys: []wireOptionKeys{
				{Table: "Lint", Checked: true, Declared: []wireOptionKey{boolean(optionsEnabled)}},
				{Table: "Format", Checked: true, Declared: []wireOptionKey{
					boolean(optionsEnabled), boolean(optionsGofumpt), boolean(optionsExtra), boolean(optionsGroupImports),
					text(optionsLocalPrefixes), text(optionsLangVersion), text(optionsModulePath),
				}},
				{Table: "CrossCuttingLint", Checked: true, Declared: []wireOptionKey{}},
			},
			ClaimedPaths:    []string{},
			HostHook:        "go-mod",
			BatchShardFloor: 2,
		}},
	}, nil
}
