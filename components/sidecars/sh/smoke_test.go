package main

import (
	"bufio"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// TestBuiltExecutableDescribesAndFormats drives the cross-compiled executable
// named by POLY_SH_BINARY over its real stdin and stdout, the way the host
// does: the describe handshake, then protocol-2 calls, each answered before
// the next is sent. CI builds it natively on macOS and Windows runners with
// mise-tasks/sidecar/build.sh, so a binary that does not start on a platform
// fails there. Without the variable the test is skipped.
func TestBuiltExecutableDescribesAndFormats(t *testing.T) {
	built := os.Getenv("POLY_SH_BINARY")
	if built == "" {
		t.Skip("POLY_SH_BINARY is not set")
	}
	dir := t.TempDir()
	exe := filepath.Join(dir, filepath.Base(built))
	data, err := os.ReadFile(built) //nolint:gosec // the executable under test, named by CI
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(exe, data, 0o700); err != nil { //nolint:gosec // an executable must be runnable
		t.Fatal(err)
	}
	manifest, err := json.Marshal(testManifest())
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, manifestFile), manifest, 0o600); err != nil {
		t.Fatal(err)
	}

	cmd := exec.Command(exe) //nolint:gosec // the executable under test
	cmd.Env = append(os.Environ(), stdioProtocolsEnv+"=1,2")
	cmd.Stderr = os.Stderr
	stdin, err := cmd.StdinPipe()
	if err != nil {
		t.Fatal(err)
	}
	stdoutPipe, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if err := cmd.Start(); err != nil {
		t.Fatalf("start %s: %v", exe, err)
	}
	timer := time.AfterFunc(30*time.Second, func() { _ = cmd.Process.Kill() })
	defer timer.Stop()
	stdout := bufio.NewReader(stdoutPipe)
	send := func(frame []byte) {
		t.Helper()
		if _, err := stdin.Write(frame); err != nil {
			t.Fatalf("write to the executable: %v", err)
		}
	}
	answerTo := func(id uint64) answer {
		t.Helper()
		got, err := readAnswer(stdout)
		if err != nil || got.id != id {
			t.Fatalf("answer = %+v, %v, want one for call %d", got, err, id)
		}
		return got
	}

	send(describeRequest(t))
	status, payload := readHandshake(t, stdout)
	var desc stdioDescriptor
	if status != statusOK || json.Unmarshal(payload, &desc) != nil || desc.ComponentID != "sh" || desc.ProtocolVersion != binaryProtocol {
		t.Fatalf("describe = status %d %q", status, payload)
	}
	send(registerCall(t, 1, 1))
	if registered := answerTo(1); registered.status != statusOK {
		t.Fatalf("RegisterConfig = status %d %q", registered.status, registered.payload)
	}
	send(formatCall(t, 2, 1, "if true; then\necho   hi\nfi\n"))
	if formatted := answerTo(2); formatted.status != statusOK || !strings.Contains(string(formatted.payload), "echo hi") {
		t.Fatalf("Format = status %d %q, want the reindented script", formatted.status, formatted.payload)
	}
	_ = stdin.Close()
	if err := cmd.Wait(); err != nil {
		t.Fatalf("the executable exited with %v after stdin closed", err)
	}
}
