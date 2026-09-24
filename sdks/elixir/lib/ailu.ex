defmodule Ailu do
  @moduledoc """
  Thin Elixir SDK over the Ailu C ABI through a small NIF.
  """

  @on_load :load_nif

  def load_nif do
    priv_dir =
      case :code.priv_dir(:ailu) do
        {:error, _reason} -> ~c"priv"
        path -> path
      end

    path = :filename.join(priv_dir, ~c"ailu_nif")
    :erlang.load_nif(path, 0)
  end

  def engine_version, do: :erlang.nif_error(:nif_not_loaded)
  def validate_graph_json(_definition_json), do: :erlang.nif_error(:nif_not_loaded)
  def compile_graph_yaml_json(_yaml), do: :erlang.nif_error(:nif_not_loaded)
  def available_providers_json, do: :erlang.nif_error(:nif_not_loaded)
  def resolve_model_json(_tier, _available_json \\ nil, _override_json \\ nil), do: :erlang.nif_error(:nif_not_loaded)
  def list_components_json, do: :erlang.nif_error(:nif_not_loaded)
  def list_prebuilt_json, do: :erlang.nif_error(:nif_not_loaded)
  def run_component_json(_kind, _params_json, _channels_json), do: :erlang.nif_error(:nif_not_loaded)
  def run_prebuilt_json(_name, _input_json, _options_json \\ nil), do: :erlang.nif_error(:nif_not_loaded)
end
