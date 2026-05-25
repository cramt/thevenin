package tree_sitter_cirq_test

import (
	"testing"

	tree_sitter "github.com/tree-sitter/go-tree-sitter"
	tree_sitter_cirq "github.com/tree-sitter/tree-sitter-cirq/bindings/go"
)

func TestCanLoadGrammar(t *testing.T) {
	language := tree_sitter.NewLanguage(tree_sitter_cirq.Language())
	if language == nil {
		t.Errorf("Error loading Cirq grammar")
	}
}
