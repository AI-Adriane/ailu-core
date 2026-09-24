# Ailu Ruby SDK

Ruby wrapper over `ailu-c-api` using the `ffi` gem.

```ruby
require "ailu"

puts Ailu.engine_version
puts Ailu.list_components_json
```

Set `AILU_C_API_LIB` to the built dynamic library when it is not on the
system loader path.
