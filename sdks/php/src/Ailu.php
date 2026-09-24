<?php

final class Ailu
{
    private \FFI $ffi;

    private function __construct(\FFI $ffi)
    {
        $this->ffi = $ffi;
    }

    public static function load(?string $library = null): self
    {
        $library = $library ?: self::defaultLibraryName();
        $cdef = <<<'CDEF'
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
CDEF;
        return new self(\FFI::cdef($cdef, $library));
    }

    public function engineVersion(): string
    {
        $ptr = $this->ffi->ailu_engine_version();
        if ($ptr === null) {
            throw new \RuntimeException("ailu_engine_version returned null");
        }

        try {
            return \FFI::string($ptr);
        } finally {
            $this->ffi->ailu_string_free($ptr);
        }
    }

    public function validateGraphJson(string $definitionJson): string
    {
        return $this->unwrap($this->ffi->ailu_validate_graph_json($definitionJson));
    }

    public function compileGraphYamlJson(string $yaml): string
    {
        return $this->unwrap($this->ffi->ailu_compile_graph_yaml_json($yaml));
    }

    public function availableProvidersJson(): string
    {
        return $this->unwrap($this->ffi->ailu_available_providers_json());
    }

    public function resolveModelJson(string $tier, ?string $availableJson = null, ?string $overrideJson = null): string
    {
        return $this->unwrap($this->ffi->ailu_resolve_model_json($tier, $availableJson, $overrideJson));
    }

    public function listComponentsJson(): string
    {
        return $this->unwrap($this->ffi->ailu_list_components_json());
    }

    public function listPrebuiltJson(): string
    {
        return $this->unwrap($this->ffi->ailu_list_prebuilt_json());
    }

    public function runComponentJson(string $kind, string $paramsJson, string $channelsJson): string
    {
        return $this->unwrap($this->ffi->ailu_run_component_json($kind, $paramsJson, $channelsJson));
    }

    public function runPrebuiltJson(string $name, string $inputJson, ?string $optionsJson = null): string
    {
        return $this->unwrap($this->ffi->ailu_run_prebuilt_json($name, $inputJson, $optionsJson));
    }

    public function engineRunJson(string $specJson, \FFI\CData $callbacks): string
    {
        return $this->unwrap($this->ffi->ailu_engine_run_json($specJson, $callbacks));
    }

    public function engineResumeJson(string $specJson, \FFI\CData $callbacks): string
    {
        return $this->unwrap($this->ffi->ailu_engine_resume_json($specJson, $callbacks));
    }

    public function engineApproveAndResumeJson(string $specJson, \FFI\CData $callbacks): string
    {
        return $this->unwrap($this->ffi->ailu_engine_approve_and_resume_json($specJson, $callbacks));
    }

    public function engineSignalJson(string $specJson, string $signalName, string $payloadJson, \FFI\CData $callbacks): string
    {
        return $this->unwrap($this->ffi->ailu_engine_signal_json($specJson, $signalName, $payloadJson, $callbacks));
    }

    public function engineReplayJson(string $specJson, string $checkpointId, \FFI\CData $callbacks): string
    {
        return $this->unwrap($this->ffi->ailu_engine_replay_json($specJson, $checkpointId, $callbacks));
    }

    private function unwrap(\FFI\CData $result): string
    {
        try {
            if ($result->code === 0) {
                return \FFI::string($result->value);
            }

            $message = $result->error === null
                ? "Ailu C API error {$result->code}"
                : \FFI::string($result->error);
            throw new \RuntimeException($message);
        } finally {
            $this->ffi->ailu_result_free($result);
        }
    }

    private static function defaultLibraryName(): string
    {
        return match (PHP_OS_FAMILY) {
            "Darwin" => "libailu_c_api.dylib",
            "Windows" => "ailu_c_api.dll",
            default => "libailu_c_api.so",
        };
    }
}
