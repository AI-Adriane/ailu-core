#include "erl_nif.h"
#include "ailu.h"
#include <string.h>

static ERL_NIF_TERM make_error(ErlNifEnv *env, const char *message) {
  return enif_make_tuple2(env, enif_make_atom(env, "error"), enif_make_string(env, message, ERL_NIF_LATIN1));
}

static ERL_NIF_TERM make_ok_string(ErlNifEnv *env, const char *value) {
  ERL_NIF_TERM binary;
  unsigned char *data;
  size_t len = strlen(value);
  data = enif_make_new_binary(env, len, &binary);
  memcpy(data, value, len);
  return enif_make_tuple2(env, enif_make_atom(env, "ok"), binary);
}

static ERL_NIF_TERM unwrap(ErlNifEnv *env, AiluResult result) {
  if (result.code == AILU_OK) {
    ERL_NIF_TERM term = make_ok_string(env, result.value == NULL ? "" : result.value);
    ailu_result_free(result);
    return term;
  }
  ERL_NIF_TERM term = make_error(env, result.error == NULL ? "Ailu C API error" : result.error);
  ailu_result_free(result);
  return term;
}

static int get_string(ErlNifEnv *env, ERL_NIF_TERM term, char **out) {
  ErlNifBinary binary;
  if (!enif_inspect_binary(env, term, &binary)) {
    return 0;
  }
  *out = enif_alloc(binary.size + 1);
  memcpy(*out, binary.data, binary.size);
  (*out)[binary.size] = '\0';
  return 1;
}

static int get_optional_string(ErlNifEnv *env, ERL_NIF_TERM term, char **out) {
  if (enif_is_identical(term, enif_make_atom(env, "nil"))) {
    *out = NULL;
    return 1;
  }
  return get_string(env, term, out);
}

static ERL_NIF_TERM engine_version(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[]) {
  (void)argc;
  (void)argv;
  char *ptr = ailu_engine_version();
  if (ptr == NULL) {
    return make_error(env, "ailu_engine_version returned null");
  }
  ERL_NIF_TERM term = make_ok_string(env, ptr);
  ailu_string_free(ptr);
  return term;
}

static ERL_NIF_TERM validate_graph_json(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[]) {
  char *input;
  if (argc != 1 || !get_string(env, argv[0], &input)) {
    return enif_make_badarg(env);
  }
  AiluResult result = ailu_validate_graph_json(input);
  enif_free(input);
  return unwrap(env, result);
}

static ERL_NIF_TERM compile_graph_yaml_json(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[]) {
  char *input;
  if (argc != 1 || !get_string(env, argv[0], &input)) {
    return enif_make_badarg(env);
  }
  AiluResult result = ailu_compile_graph_yaml_json(input);
  enif_free(input);
  return unwrap(env, result);
}

static ERL_NIF_TERM available_providers_json(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[]) {
  (void)argc;
  (void)argv;
  return unwrap(env, ailu_available_providers_json());
}

static ERL_NIF_TERM resolve_model_json(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[]) {
  char *tier;
  char *available;
  char *override_json;
  if (argc != 3 || !get_string(env, argv[0], &tier) || !get_optional_string(env, argv[1], &available) || !get_optional_string(env, argv[2], &override_json)) {
    return enif_make_badarg(env);
  }
  AiluResult result = ailu_resolve_model_json(tier, available, override_json);
  enif_free(tier);
  if (available != NULL) enif_free(available);
  if (override_json != NULL) enif_free(override_json);
  return unwrap(env, result);
}

static ERL_NIF_TERM list_components_json(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[]) {
  (void)argc;
  (void)argv;
  return unwrap(env, ailu_list_components_json());
}

static ERL_NIF_TERM list_prebuilt_json(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[]) {
  (void)argc;
  (void)argv;
  return unwrap(env, ailu_list_prebuilt_json());
}

static ERL_NIF_TERM run_component_json(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[]) {
  char *kind;
  char *params;
  char *channels;
  if (argc != 3 || !get_string(env, argv[0], &kind) || !get_string(env, argv[1], &params) || !get_string(env, argv[2], &channels)) {
    return enif_make_badarg(env);
  }
  AiluResult result = ailu_run_component_json(kind, params, channels);
  enif_free(kind);
  enif_free(params);
  enif_free(channels);
  return unwrap(env, result);
}

static ERL_NIF_TERM run_prebuilt_json(ErlNifEnv *env, int argc, const ERL_NIF_TERM argv[]) {
  char *name;
  char *input;
  char *options;
  if (argc != 3 || !get_string(env, argv[0], &name) || !get_string(env, argv[1], &input) || !get_optional_string(env, argv[2], &options)) {
    return enif_make_badarg(env);
  }
  AiluResult result = ailu_run_prebuilt_json(name, input, options);
  enif_free(name);
  enif_free(input);
  if (options != NULL) enif_free(options);
  return unwrap(env, result);
}

static ErlNifFunc funcs[] = {
  {"engine_version", 0, engine_version},
  {"validate_graph_json", 1, validate_graph_json},
  {"compile_graph_yaml_json", 1, compile_graph_yaml_json},
  {"available_providers_json", 0, available_providers_json},
  {"resolve_model_json", 3, resolve_model_json},
  {"list_components_json", 0, list_components_json},
  {"list_prebuilt_json", 0, list_prebuilt_json},
  {"run_component_json", 3, run_component_json},
  {"run_prebuilt_json", 3, run_prebuilt_json},
};

ERL_NIF_INIT(Elixir.Ailu, funcs, NULL, NULL, NULL, NULL)
