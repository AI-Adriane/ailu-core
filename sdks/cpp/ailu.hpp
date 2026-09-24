#pragma once

#include <stdexcept>
#include <string>

#include "../../crates/c-api/include/ailu.h"

namespace ailu {

class Error : public std::runtime_error {
 public:
  using std::runtime_error::runtime_error;
};

inline std::string take_string(char* ptr) {
  if (ptr == nullptr) {
    return {};
  }
  std::string value(ptr);
  ailu_string_free(ptr);
  return value;
}

inline std::string unwrap(AiluResult result) {
  if (result.code == AILU_OK) {
    std::string value = result.value == nullptr ? std::string{} : std::string(result.value);
    ailu_result_free(result);
    return value;
  }

  std::string message = result.error == nullptr
      ? "Ailu C API error " + std::to_string(result.code)
      : std::string(result.error);
  ailu_result_free(result);
  throw Error(message);
}

inline std::string engine_version() {
  return take_string(ailu_engine_version());
}

inline std::string validate_graph_json(const std::string& definition_json) {
  return unwrap(ailu_validate_graph_json(definition_json.c_str()));
}

inline std::string compile_graph_yaml_json(const std::string& yaml) {
  return unwrap(ailu_compile_graph_yaml_json(yaml.c_str()));
}

inline std::string available_providers_json() {
  return unwrap(ailu_available_providers_json());
}

inline std::string resolve_model_json(
    const std::string& tier,
    const char* available_json = nullptr,
    const char* override_json = nullptr) {
  return unwrap(ailu_resolve_model_json(tier.c_str(), available_json, override_json));
}

inline std::string list_components_json() {
  return unwrap(ailu_list_components_json());
}

inline std::string list_prebuilt_json() {
  return unwrap(ailu_list_prebuilt_json());
}

inline std::string run_component_json(
    const std::string& kind,
    const std::string& params_json,
    const std::string& channels_json) {
  return unwrap(ailu_run_component_json(kind.c_str(), params_json.c_str(), channels_json.c_str()));
}

inline std::string run_prebuilt_json(
    const std::string& name,
    const std::string& input_json,
    const char* options_json = nullptr) {
  return unwrap(ailu_run_prebuilt_json(name.c_str(), input_json.c_str(), options_json));
}

inline std::string engine_run_json(const std::string& spec_json, AiluCallbacks callbacks) {
  return unwrap(ailu_engine_run_json(spec_json.c_str(), callbacks));
}

inline std::string engine_resume_json(const std::string& spec_json, AiluCallbacks callbacks) {
  return unwrap(ailu_engine_resume_json(spec_json.c_str(), callbacks));
}

inline std::string engine_approve_and_resume_json(const std::string& spec_json, AiluCallbacks callbacks) {
  return unwrap(ailu_engine_approve_and_resume_json(spec_json.c_str(), callbacks));
}

inline std::string engine_signal_json(
    const std::string& spec_json,
    const std::string& signal_name,
    const std::string& payload_json,
    AiluCallbacks callbacks) {
  return unwrap(ailu_engine_signal_json(spec_json.c_str(), signal_name.c_str(), payload_json.c_str(), callbacks));
}

inline std::string engine_replay_json(
    const std::string& spec_json,
    const std::string& checkpoint_id,
    AiluCallbacks callbacks) {
  return unwrap(ailu_engine_replay_json(spec_json.c_str(), checkpoint_id.c_str(), callbacks));
}

}  // namespace ailu
