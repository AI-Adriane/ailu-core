# Ailu Go SDK

Go cgo wrapper over `ailu-c-api`.

```go
package main

import (
	"fmt"

	"ailu/sdks/go/ailu"
)

func main() {
	fmt.Println(ailu.EngineVersion())
}
```

Build `ailu-c-api` first and make the dynamic library discoverable with
`DYLD_LIBRARY_PATH`, `LD_LIBRARY_PATH`, or your platform equivalent.
