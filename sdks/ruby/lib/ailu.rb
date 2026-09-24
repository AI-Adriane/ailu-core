# frozen_string_literal: true

require "ffi"

module Ailu
  extend FFI::Library

  ffi_lib ENV.fetch("AILU_C_API_LIB", "ailu_c_api")

  class Result < FFI::Struct
    layout :code, :int,
           :value, :pointer,
           :error, :pointer
  end

  callback :string_callback, [:string, :pointer, :pointer, :pointer], :int
  callback :event_callback, [:string, :pointer], :void

  class Callbacks < FFI::Struct
    layout :user_data, :pointer,
           :on_node, :string_callback,
           :on_condition, :string_callback,
           :on_event, :event_callback
  end

  attach_function :native_engine_version, :ailu_engine_version, [], :pointer
  attach_function :native_validate_graph_json, :ailu_validate_graph_json, [:string], Result.by_value
  attach_function :native_compile_graph_yaml_json, :ailu_compile_graph_yaml_json, [:string], Result.by_value
  attach_function :native_available_providers_json, :ailu_available_providers_json, [], Result.by_value
  attach_function :native_resolve_model_json, :ailu_resolve_model_json, [:string, :pointer, :pointer], Result.by_value
  attach_function :native_list_components_json, :ailu_list_components_json, [], Result.by_value
  attach_function :native_list_prebuilt_json, :ailu_list_prebuilt_json, [], Result.by_value
  attach_function :native_run_component_json, :ailu_run_component_json, [:string, :string, :string], Result.by_value
  attach_function :native_run_prebuilt_json, :ailu_run_prebuilt_json, [:string, :string, :pointer], Result.by_value
  attach_function :native_engine_run_json, :ailu_engine_run_json, [:string, Callbacks.by_value], Result.by_value
  attach_function :native_engine_resume_json, :ailu_engine_resume_json, [:string, Callbacks.by_value], Result.by_value
  attach_function :native_engine_approve_and_resume_json, :ailu_engine_approve_and_resume_json, [:string, Callbacks.by_value], Result.by_value
  attach_function :native_engine_signal_json, :ailu_engine_signal_json, [:string, :string, :string, Callbacks.by_value], Result.by_value
  attach_function :native_engine_replay_json, :ailu_engine_replay_json, [:string, :string, Callbacks.by_value], Result.by_value
  attach_function :native_string_free, :ailu_string_free, [:pointer], :void
  attach_function :native_result_free, :ailu_result_free, [Result.by_value], :void

  module_function

  def engine_version
    ptr = native_engine_version
    raise Error, "ailu_engine_version returned null" if ptr.null?

    ptr.read_string
  ensure
    native_string_free(ptr) if ptr && !ptr.null?
  end

  def validate_graph_json(definition_json)
    unwrap(native_validate_graph_json(definition_json))
  end

  def compile_graph_yaml_json(yaml)
    unwrap(native_compile_graph_yaml_json(yaml))
  end

  def available_providers_json
    unwrap(native_available_providers_json)
  end

  def resolve_model_json(tier, available_json: nil, override_json: nil)
    available = optional_string_pointer(available_json)
    override = optional_string_pointer(override_json)
    unwrap(native_resolve_model_json(tier, available, override))
  end

  def list_components_json
    unwrap(native_list_components_json)
  end

  def list_prebuilt_json
    unwrap(native_list_prebuilt_json)
  end

  def run_component_json(kind, params_json, channels_json)
    unwrap(native_run_component_json(kind, params_json, channels_json))
  end

  def run_prebuilt_json(name, input_json, options_json: nil)
    options = optional_string_pointer(options_json)
    unwrap(native_run_prebuilt_json(name, input_json, options))
  end

  def engine_run_json(spec_json, callbacks)
    unwrap(native_engine_run_json(spec_json, callbacks))
  end

  def engine_resume_json(spec_json, callbacks)
    unwrap(native_engine_resume_json(spec_json, callbacks))
  end

  def engine_approve_and_resume_json(spec_json, callbacks)
    unwrap(native_engine_approve_and_resume_json(spec_json, callbacks))
  end

  def engine_signal_json(spec_json, signal_name, payload_json, callbacks)
    unwrap(native_engine_signal_json(spec_json, signal_name, payload_json, callbacks))
  end

  def engine_replay_json(spec_json, checkpoint_id, callbacks)
    unwrap(native_engine_replay_json(spec_json, checkpoint_id, callbacks))
  end

  def optional_string_pointer(value)
    return FFI::Pointer::NULL if value.nil?

    FFI::MemoryPointer.from_string(value)
  end
  private_class_method :optional_string_pointer

  def unwrap(result)
    if result[:code].zero?
      return result[:value].read_string
    end

    message = result[:error].null? ? "Ailu C API error #{result[:code]}" : result[:error].read_string
    raise Error, message
  ensure
    native_result_free(result)
  end
  private_class_method :unwrap

  class Error < StandardError; end
end
