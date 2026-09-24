using System;
using System.Collections.Concurrent;
using System.Runtime.InteropServices;

namespace Ailu;

public sealed class AiluException : Exception
{
    public int Code { get; }

    public AiluException(int code, string message) : base(message)
    {
        Code = code;
    }
}

public static class Ailu
{
    [StructLayout(LayoutKind.Sequential)]
    private struct AiluResult
    {
        public int Code;
        public IntPtr Value;
        public IntPtr Error;
    }

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate int AiluStringCallback(
        IntPtr payloadJson,
        IntPtr userData,
        out IntPtr value,
        out IntPtr error);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    internal delegate void AiluEventCallback(IntPtr payloadJson, IntPtr userData);

    [StructLayout(LayoutKind.Sequential)]
    internal struct AiluCallbacks
    {
        public IntPtr UserData;
        public AiluStringCallback? OnNode;
        public AiluStringCallback? OnCondition;
        public AiluEventCallback? OnEvent;
    }

    public sealed class EngineCallbacks : IDisposable
    {
        private readonly ConcurrentBag<IntPtr> allocations = new();
        private readonly Func<string, string> onNode;
        private readonly Func<string, string> onCondition;
        private readonly Action<string>? onEvent;

        private readonly AiluStringCallback nativeNode;
        private readonly AiluStringCallback nativeCondition;
        private readonly AiluEventCallback nativeEvent;

        public EngineCallbacks(
            Func<string, string> onNode,
            Func<string, string> onCondition,
            Action<string>? onEvent = null)
        {
            this.onNode = onNode;
            this.onCondition = onCondition;
            this.onEvent = onEvent;
            nativeNode = (IntPtr payload, IntPtr userData, out IntPtr value, out IntPtr error) =>
                InvokeString(payload, this.onNode, out value, out error);
            nativeCondition = (IntPtr payload, IntPtr userData, out IntPtr value, out IntPtr error) =>
                InvokeString(payload, this.onCondition, out value, out error);
            nativeEvent = (payload, _) => this.onEvent?.Invoke(Marshal.PtrToStringUTF8(payload) ?? string.Empty);
        }

        internal AiluCallbacks ToNative() => new()
        {
            UserData = IntPtr.Zero,
            OnNode = nativeNode,
            OnCondition = nativeCondition,
            OnEvent = nativeEvent,
        };

        private int InvokeString(IntPtr payload, Func<string, string> callback, out IntPtr value, out IntPtr error)
        {
            try
            {
                var result = callback(Marshal.PtrToStringUTF8(payload) ?? string.Empty);
                var ptr = Marshal.StringToCoTaskMemUTF8(result);
                allocations.Add(ptr);
                value = ptr;
                error = IntPtr.Zero;
                return 0;
            }
            catch (Exception ex)
            {
                var ptr = Marshal.StringToCoTaskMemUTF8(ex.Message);
                allocations.Add(ptr);
                value = IntPtr.Zero;
                error = ptr;
                return 3;
            }
        }

        public void Dispose()
        {
            while (allocations.TryTake(out var ptr))
            {
                Marshal.FreeCoTaskMem(ptr);
            }
        }
    }

    private const string Library = "ailu_c_api";

    static Ailu()
    {
        var path = Environment.GetEnvironmentVariable("AILU_C_API_LIB");
        if (string.IsNullOrWhiteSpace(path))
        {
            return;
        }

        NativeLibrary.SetDllImportResolver(typeof(Ailu).Assembly, (libraryName, assembly, searchPath) =>
        {
            return libraryName == Library ? NativeLibrary.Load(path) : IntPtr.Zero;
        });
    }

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr ailu_engine_version();

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_validate_graph_json(string definitionJson);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_compile_graph_yaml_json(string yaml);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_available_providers_json();

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_resolve_model_json(string tier, string? availableJson, string? overrideJson);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_list_components_json();

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_list_prebuilt_json();

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_run_component_json(string kind, string paramsJson, string channelsJson);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_run_prebuilt_json(string name, string inputJson, string? optionsJson);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_engine_run_json(string specJson, AiluCallbacks callbacks);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_engine_resume_json(string specJson, AiluCallbacks callbacks);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_engine_approve_and_resume_json(string specJson, AiluCallbacks callbacks);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_engine_signal_json(string specJson, string signalName, string payloadJson, AiluCallbacks callbacks);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern AiluResult ailu_engine_replay_json(string specJson, string checkpointId, AiluCallbacks callbacks);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern void ailu_string_free(IntPtr ptr);

    [DllImport(Library, CallingConvention = CallingConvention.Cdecl)]
    private static extern void ailu_result_free(AiluResult result);

    public static string EngineVersion()
    {
        var ptr = ailu_engine_version();
        if (ptr == IntPtr.Zero)
        {
            return string.Empty;
        }

        try
        {
            return Marshal.PtrToStringUTF8(ptr) ?? string.Empty;
        }
        finally
        {
            ailu_string_free(ptr);
        }
    }

    public static string ValidateGraphJson(string definitionJson) => Unwrap(ailu_validate_graph_json(definitionJson));

    public static string CompileGraphYamlJson(string yaml) => Unwrap(ailu_compile_graph_yaml_json(yaml));

    public static string AvailableProvidersJson() => Unwrap(ailu_available_providers_json());

    public static string ResolveModelJson(string tier, string? availableJson = null, string? overrideJson = null) =>
        Unwrap(ailu_resolve_model_json(tier, availableJson, overrideJson));

    public static string ListComponentsJson() => Unwrap(ailu_list_components_json());

    public static string ListPrebuiltJson() => Unwrap(ailu_list_prebuilt_json());

    public static string RunComponentJson(string kind, string paramsJson, string channelsJson) =>
        Unwrap(ailu_run_component_json(kind, paramsJson, channelsJson));

    public static string RunPrebuiltJson(string name, string inputJson, string? optionsJson = null) =>
        Unwrap(ailu_run_prebuilt_json(name, inputJson, optionsJson));

    public static string EngineRunJson(string specJson, EngineCallbacks callbacks) =>
        Unwrap(ailu_engine_run_json(specJson, callbacks.ToNative()));

    public static string EngineResumeJson(string specJson, EngineCallbacks callbacks) =>
        Unwrap(ailu_engine_resume_json(specJson, callbacks.ToNative()));

    public static string EngineApproveAndResumeJson(string specJson, EngineCallbacks callbacks) =>
        Unwrap(ailu_engine_approve_and_resume_json(specJson, callbacks.ToNative()));

    public static string EngineSignalJson(string specJson, string signalName, string payloadJson, EngineCallbacks callbacks) =>
        Unwrap(ailu_engine_signal_json(specJson, signalName, payloadJson, callbacks.ToNative()));

    public static string EngineReplayJson(string specJson, string checkpointId, EngineCallbacks callbacks) =>
        Unwrap(ailu_engine_replay_json(specJson, checkpointId, callbacks.ToNative()));

    private static string Unwrap(AiluResult result)
    {
        try
        {
            if (result.Code == 0)
            {
                return Marshal.PtrToStringUTF8(result.Value) ?? string.Empty;
            }

            var message = result.Error == IntPtr.Zero
                ? $"Ailu C API error {result.Code}"
                : Marshal.PtrToStringUTF8(result.Error) ?? $"Ailu C API error {result.Code}";
            throw new AiluException(result.Code, message);
        }
        finally
        {
            ailu_result_free(result);
        }
    }
}
