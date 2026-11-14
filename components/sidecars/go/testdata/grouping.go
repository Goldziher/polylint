package grouping

import (
	"github.com/a/b"
	"os"
	"example.com/mod/inner"
	"fmt"
)

func Use() {
	fmt.Println(b.X, os.Args, inner.Y)
}
