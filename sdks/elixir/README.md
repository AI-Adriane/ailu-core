# Ailu Elixir SDK

Elixir wrapper over `ailu-c-api` through a small C NIF.

Build the NIF against Erlang headers and link it with `ailu_c_api`, then place
the produced shared object at `priv/ailu_nif`.
