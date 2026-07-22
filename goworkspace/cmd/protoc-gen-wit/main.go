package main

import (
	"fmt"
	"os"

	"github.com/TrogonStack/trogonai/goworkspace/internal/protowit"
)

func main() {
	if err := protowit.Run(os.Stdin, os.Stdout); err != nil {
		_, _ = fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
