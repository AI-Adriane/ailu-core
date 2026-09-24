#ifndef AILU_H
#define AILU_H

#ifdef __cplusplus
extern "C" {
#endif

#define AILU_OK 0
#define AILU_ERR_NULL 1
#define AILU_ERR_UTF8 2
#define AILU_ERR_INPUT 3
#define AILU_ERR_INTERNAL 4

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

#ifdef __cplusplus
}
#endif

#endif
