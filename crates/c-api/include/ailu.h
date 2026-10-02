#ifndef AILU_H
#define AILU_H

#include <stddef.h>

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

/* Polled at every node boundary: return non-zero to stop the run there, with status "cancelled"
   and its last checkpoint intact (ADR 0044). */
typedef int (*AiluCancelCallback)(void *user_data);

/* The callbacks of the original entry points, passed by value. Its layout is frozen. */
typedef struct AiluCallbacks {
  void *user_data;
  AiluStringCallback on_node;
  AiluStringCallback on_condition;
  AiluEventCallback on_event;
} AiluCallbacks;

/* The callbacks of the _v2 entry points, passed by pointer (ADR 0045 D2.4): AiluCallbacks plus
   is_cancelled. Set struct_size to sizeof(AiluCallbacksV2): a later release appends fields and
   reads them only from callers that sent a larger size. A NULL is_cancelled never cancels. */
typedef struct AiluCallbacksV2 {
  size_t struct_size;
  void *user_data;
  AiluStringCallback on_node;
  AiluStringCallback on_condition;
  AiluEventCallback on_event;
  AiluCancelCallback is_cancelled;
} AiluCallbacksV2;

char *ailu_engine_version(void);
AiluResult ailu_validate_graph_json(const char *definition_json);
AiluResult ailu_compile_graph_yaml_json(const char *yaml);
AiluResult ailu_available_providers_json(void);
AiluResult ailu_resolve_model_json(const char *tier, const char *available_json, const char *override_json);
AiluResult ailu_list_components_json(void);
AiluResult ailu_list_prebuilt_json(void);
AiluResult ailu_run_component_json(const char *kind, const char *params_json, const char *channels_json);
AiluResult ailu_run_prebuilt_json(const char *name, const char *input_json, const char *options_json);
AiluResult ailu_spec_from_catalog_json(const char *input_json);
AiluResult ailu_catalog_approval_plan_json(const char *input_json);
AiluResult ailu_catalog_approvals_to_check_json(const char *state_json);
AiluResult ailu_catalog_resume_problems_json(const char *input_json);
AiluResult ailu_engine_run_json(const char *spec_json, AiluCallbacks callbacks);
AiluResult ailu_engine_resume_json(const char *spec_json, AiluCallbacks callbacks);
AiluResult ailu_engine_approve_and_resume_json(const char *spec_json, AiluCallbacks callbacks);
AiluResult ailu_engine_signal_json(const char *spec_json, const char *signal_name, const char *payload_json, AiluCallbacks callbacks);
AiluResult ailu_engine_replay_json(const char *spec_json, const char *checkpoint_id, AiluCallbacks callbacks);
AiluResult ailu_engine_run_json_v2(const char *spec_json, const AiluCallbacksV2 *callbacks);
AiluResult ailu_engine_resume_json_v2(const char *spec_json, const AiluCallbacksV2 *callbacks);
AiluResult ailu_engine_approve_and_resume_json_v2(const char *spec_json, const AiluCallbacksV2 *callbacks);
AiluResult ailu_engine_signal_json_v2(const char *spec_json, const char *signal_name, const char *payload_json, const AiluCallbacksV2 *callbacks);
void ailu_string_free(char *ptr);
void ailu_result_free(AiluResult result);

#ifdef __cplusplus
}
#endif

#endif
