local ffi = require("ffi")

ffi.cdef[[
typedef struct AiluResult {
  int code;
  char *value;
  char *error;
} AiluResult;

typedef int (*AiluStringCallback)(const char *payload_json, void *user_data, const char **value, const char **error);
typedef void (*AiluEventCallback)(const char *payload_json, void *user_data);

typedef struct AiluCallbacks {
  void *user_data;
  AiluStringCallback on_node;
  AiluStringCallback on_condition;
  AiluEventCallback on_event;
} AiluCallbacks;

char *ailu_engine_version(void);
AiluResult ailu_validate_graph_json(const char *definition_json);
AiluResult ailu_compile_graph_yaml_json(const char *yaml);
AiluResult ailu_available_providers_json(void);
AiluResult ailu_resolve_model_json(const char *tier, const char *available_json, const char *override_json);
AiluResult ailu_list_components_json(void);
AiluResult ailu_list_prebuilt_json(void);
AiluResult ailu_run_component_json(const char *kind, const char *params_json, const char *channels_json);
AiluResult ailu_run_prebuilt_json(const char *name, const char *input_json, const char *options_json);
AiluResult ailu_engine_run_json(const char *spec_json, AiluCallbacks callbacks);
AiluResult ailu_engine_resume_json(const char *spec_json, AiluCallbacks callbacks);
AiluResult ailu_engine_approve_and_resume_json(const char *spec_json, AiluCallbacks callbacks);
AiluResult ailu_engine_signal_json(const char *spec_json, const char *signal_name, const char *payload_json, AiluCallbacks callbacks);
AiluResult ailu_engine_replay_json(const char *spec_json, const char *checkpoint_id, AiluCallbacks callbacks);
void ailu_string_free(char *ptr);
void ailu_result_free(AiluResult result);
]]

local M = {}
local lib = nil

local function default_library_name()
  if ffi.os == "OSX" then
    return "libailu_c_api.dylib"
  end
  if ffi.os == "Windows" then
    return "ailu_c_api.dll"
  end
  return "libailu_c_api.so"
end

function M.load(path)
  lib = ffi.load(path or os.getenv("AILU_C_API_LIB") or default_library_name())
  return M
end

local function engine()
  if lib == nil then
    M.load()
  end
  return lib
end

local function take_string(ptr)
  if ptr == nil or ptr == ffi.NULL then
    return nil
  end
  local value = ffi.string(ptr)
  engine().ailu_string_free(ptr)
  return value
end

local function unwrap(result)
  local native = engine()
  if result.code == 0 then
    local value = ffi.string(result.value)
    native.ailu_result_free(result)
    return value
  end

  local message = "Ailu C API error " .. tostring(result.code)
  if result.error ~= nil and result.error ~= ffi.NULL then
    message = ffi.string(result.error)
  end
  native.ailu_result_free(result)
  error(message, 2)
end

function M.engine_version()
  local ptr = engine().ailu_engine_version()
  if ptr == nil or ptr == ffi.NULL then
    error("ailu_engine_version returned null", 2)
  end
  return take_string(ptr)
end

function M.validate_graph_json(definition_json)
  return unwrap(engine().ailu_validate_graph_json(definition_json))
end

function M.compile_graph_yaml_json(yaml)
  return unwrap(engine().ailu_compile_graph_yaml_json(yaml))
end

function M.available_providers_json()
  return unwrap(engine().ailu_available_providers_json())
end

function M.resolve_model_json(tier, available_json, override_json)
  return unwrap(engine().ailu_resolve_model_json(tier, available_json, override_json))
end

function M.list_components_json()
  return unwrap(engine().ailu_list_components_json())
end

function M.list_prebuilt_json()
  return unwrap(engine().ailu_list_prebuilt_json())
end

function M.run_component_json(kind, params_json, channels_json)
  return unwrap(engine().ailu_run_component_json(kind, params_json, channels_json))
end

function M.run_prebuilt_json(name, input_json, options_json)
  return unwrap(engine().ailu_run_prebuilt_json(name, input_json, options_json))
end

function M.engine_run_json(spec_json, callbacks)
  return unwrap(engine().ailu_engine_run_json(spec_json, callbacks))
end

function M.engine_resume_json(spec_json, callbacks)
  return unwrap(engine().ailu_engine_resume_json(spec_json, callbacks))
end

function M.engine_approve_and_resume_json(spec_json, callbacks)
  return unwrap(engine().ailu_engine_approve_and_resume_json(spec_json, callbacks))
end

function M.engine_signal_json(spec_json, signal_name, payload_json, callbacks)
  return unwrap(engine().ailu_engine_signal_json(spec_json, signal_name, payload_json, callbacks))
end

function M.engine_replay_json(spec_json, checkpoint_id, callbacks)
  return unwrap(engine().ailu_engine_replay_json(spec_json, checkpoint_id, callbacks))
end

return M
