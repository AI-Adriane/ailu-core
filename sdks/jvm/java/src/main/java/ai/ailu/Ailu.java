package ai.ailu;

import com.sun.jna.Callback;
import com.sun.jna.Library;
import com.sun.jna.Memory;
import com.sun.jna.Native;
import com.sun.jna.Pointer;
import com.sun.jna.Structure;
import com.sun.jna.ptr.PointerByReference;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

public final class Ailu {
  private Ailu() {}

  public static final class AiluException extends RuntimeException {
    public final int code;

    public AiluException(int code, String message) {
      super(message);
      this.code = code;
    }
  }

  public static final class AiluResult extends Structure implements Structure.ByValue {
    public int code;
    public Pointer value;
    public Pointer error;

    @Override
    protected List<String> getFieldOrder() {
      return Arrays.asList("code", "value", "error");
    }
  }

  public static final class AiluCallbackResult {
    private static final List<Memory> CALLBACK_STRINGS = Collections.synchronizedList(new ArrayList<>());

    public final int code;
    public final Pointer value;
    public final Pointer error;

    private AiluCallbackResult(int code, Pointer value, Pointer error) {
      this.code = code;
      this.value = value;
      this.error = error;
    }

    public static AiluCallbackResult ok(String value) {
      return new AiluCallbackResult(0, callbackString(value), null);
    }

    public static AiluCallbackResult error(String message) {
      return new AiluCallbackResult(3, null, callbackString(message));
    }

    int writeTo(PointerByReference valueOut, PointerByReference errorOut) {
      valueOut.setValue(value);
      errorOut.setValue(error);
      return code;
    }

    private static Pointer callbackString(String value) {
      if (value == null) {
        value = "";
      }
      byte[] bytes = value.getBytes(StandardCharsets.UTF_8);
      Memory memory = new Memory(bytes.length + 1L);
      memory.write(0, bytes, 0, bytes.length);
      memory.setByte(bytes.length, (byte) 0);
      CALLBACK_STRINGS.add(memory);
      return memory;
    }
  }

  @FunctionalInterface
  public interface HostStringCallback {
    AiluCallbackResult invoke(String payloadJson, Pointer userData);
  }

  public interface StringCallback extends Callback {
    int invoke(String payloadJson, Pointer userData, PointerByReference value, PointerByReference error);
  }

  public interface EventCallback extends Callback {
    void invoke(String payloadJson, Pointer userData);
  }

  public static final class AiluCallbacks extends Structure implements Structure.ByValue {
    public Pointer userData;
    public StringCallback onNode;
    public StringCallback onCondition;
    public EventCallback onEvent;

    public AiluCallbacks() {
      this.userData = null;
    }

    public AiluCallbacks(HostStringCallback onNode, HostStringCallback onCondition, EventCallback onEvent) {
      this.userData = null;
      this.onNode = stringCallback(onNode);
      this.onCondition = stringCallback(onCondition);
      this.onEvent = onEvent;
    }

    @Override
    protected List<String> getFieldOrder() {
      return Arrays.asList("userData", "onNode", "onCondition", "onEvent");
    }
  }

  private static StringCallback stringCallback(HostStringCallback callback) {
    return (payloadJson, userData, value, error) -> {
      try {
        AiluCallbackResult result = callback.invoke(payloadJson, userData);
        if (result == null) {
          result = AiluCallbackResult.error("callback returned null");
        }
        return result.writeTo(value, error);
      } catch (RuntimeException ex) {
        String message = ex.getMessage() == null ? ex.getClass().getSimpleName() : ex.getMessage();
        return AiluCallbackResult.error(message).writeTo(value, error);
      }
    };
  }

  private interface NativeAilu extends Library {
    NativeAilu INSTANCE = Native.load(
        System.getenv().getOrDefault("AILU_C_API_LIB", "ailu_c_api"),
        NativeAilu.class
    );

    Pointer ailu_engine_version();
    AiluResult ailu_validate_graph_json(String definitionJson);
    AiluResult ailu_compile_graph_yaml_json(String yaml);
    AiluResult ailu_available_providers_json();
    AiluResult ailu_resolve_model_json(String tier, String availableJson, String overrideJson);
    AiluResult ailu_list_components_json();
    AiluResult ailu_list_prebuilt_json();
    AiluResult ailu_run_component_json(String kind, String paramsJson, String channelsJson);
    AiluResult ailu_run_prebuilt_json(String name, String inputJson, String optionsJson);
    AiluResult ailu_engine_run_json(String specJson, AiluCallbacks callbacks);
    AiluResult ailu_engine_resume_json(String specJson, AiluCallbacks callbacks);
    AiluResult ailu_engine_approve_and_resume_json(String specJson, AiluCallbacks callbacks);
    AiluResult ailu_engine_signal_json(String specJson, String signalName, String payloadJson, AiluCallbacks callbacks);
    AiluResult ailu_engine_replay_json(String specJson, String checkpointId, AiluCallbacks callbacks);
    void ailu_string_free(Pointer ptr);
    void ailu_result_free(AiluResult result);
  }

  public static String engineVersion() {
    Pointer ptr = NativeAilu.INSTANCE.ailu_engine_version();
    if (ptr == null) {
      return "";
    }
    try {
      return ptr.getString(0, "UTF-8");
    } finally {
      NativeAilu.INSTANCE.ailu_string_free(ptr);
    }
  }

  public static String validateGraphJson(String definitionJson) {
    return unwrap(NativeAilu.INSTANCE.ailu_validate_graph_json(definitionJson));
  }

  public static String compileGraphYamlJson(String yaml) {
    return unwrap(NativeAilu.INSTANCE.ailu_compile_graph_yaml_json(yaml));
  }

  public static String availableProvidersJson() {
    return unwrap(NativeAilu.INSTANCE.ailu_available_providers_json());
  }

  public static String resolveModelJson(String tier, String availableJson, String overrideJson) {
    return unwrap(NativeAilu.INSTANCE.ailu_resolve_model_json(tier, availableJson, overrideJson));
  }

  public static String listComponentsJson() {
    return unwrap(NativeAilu.INSTANCE.ailu_list_components_json());
  }

  public static String listPrebuiltJson() {
    return unwrap(NativeAilu.INSTANCE.ailu_list_prebuilt_json());
  }

  public static String runComponentJson(String kind, String paramsJson, String channelsJson) {
    return unwrap(NativeAilu.INSTANCE.ailu_run_component_json(kind, paramsJson, channelsJson));
  }

  public static String runPrebuiltJson(String name, String inputJson, String optionsJson) {
    return unwrap(NativeAilu.INSTANCE.ailu_run_prebuilt_json(name, inputJson, optionsJson));
  }

  public static String engineRunJson(String specJson, AiluCallbacks callbacks) {
    return unwrap(NativeAilu.INSTANCE.ailu_engine_run_json(specJson, callbacks));
  }

  public static String engineResumeJson(String specJson, AiluCallbacks callbacks) {
    return unwrap(NativeAilu.INSTANCE.ailu_engine_resume_json(specJson, callbacks));
  }

  public static String engineApproveAndResumeJson(String specJson, AiluCallbacks callbacks) {
    return unwrap(NativeAilu.INSTANCE.ailu_engine_approve_and_resume_json(specJson, callbacks));
  }

  public static String engineSignalJson(String specJson, String signalName, String payloadJson, AiluCallbacks callbacks) {
    return unwrap(NativeAilu.INSTANCE.ailu_engine_signal_json(specJson, signalName, payloadJson, callbacks));
  }

  public static String engineReplayJson(String specJson, String checkpointId, AiluCallbacks callbacks) {
    return unwrap(NativeAilu.INSTANCE.ailu_engine_replay_json(specJson, checkpointId, callbacks));
  }

  private static String unwrap(AiluResult result) {
    try {
      if (result.code == 0) {
        return result.value == null ? "" : result.value.getString(0, "UTF-8");
      }

      String message = result.error == null
          ? "Ailu C API error " + result.code
          : result.error.getString(0, "UTF-8");
      throw new AiluException(result.code, message);
    } finally {
      NativeAilu.INSTANCE.ailu_result_free(result);
    }
  }
}
