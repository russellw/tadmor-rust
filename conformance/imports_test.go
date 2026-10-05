package main

import (
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

// TestStandardLibraryOnly keeps the suite a black box that any counterpart
// can run without trusting more than the Go toolchain: no third-party modules,
// and nothing from tadmor itself.
func TestStandardLibraryOnly(t *testing.T) {
	files, err := filepath.Glob("*.go")
	if err != nil {
		t.Fatal(err)
	}
	fset := token.NewFileSet()
	for _, name := range files {
		src, err := os.ReadFile(name)
		if err != nil {
			t.Fatal(err)
		}
		f, err := parser.ParseFile(fset, name, src, parser.ImportsOnly)
		if err != nil {
			t.Fatal(err)
		}
		for _, imp := range f.Imports {
			p, _ := strconv.Unquote(imp.Path.Value)
			// Standard-library paths have no dot in their first element.
			if first, _, _ := strings.Cut(p, "/"); strings.Contains(first, ".") || first == "tadmor" {
				t.Errorf("%s imports %q; the conformance suite must use only the standard library", name, p)
			}
		}
	}
}
