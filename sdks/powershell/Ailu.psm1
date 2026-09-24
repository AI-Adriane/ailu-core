$NativeLibrary = if ($env:AILU_C_API_LIB) {
  $env:AILU_C_API_LIB.Replace("\", "\\")
} else {
  "ailu_c_api"
}

$Source = @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class AiluNative {
  [StructLayout(LayoutKind.Sequential)]
  public struct AiluResult {
    public int Code;
    public IntPtr Value;
    public IntPtr Error;
  }

  [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
  public delegate int AiluStringCallback(
    IntPtr payloadJson,
    IntPtr userData,
    out IntPtr value,
    out IntPtr error);

  [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
  public delegate void AiluEventCallback(IntPtr payloadJson, IntPtr userData);

  [StructLayout(LayoutKind.Sequential)]
  public struct AiluCallbacks {
    public IntPtr UserData;
    public AiluStringCallback OnNode;
    public AiluStringCallback OnCondition;
    public AiluEventCallback OnEvent;
  }

  public static string PtrToUtf8(IntPtr ptr) {
    if (ptr == IntPtr.Zero) {
      return "";
    }

    int len = 0;
    while (Marshal.ReadByte(ptr, len) != 0) {
      len++;
    }

    byte[] bytes = new byte[len];
    Marshal.Copy(ptr, bytes, 0, len);
    return Encoding.UTF8.GetString(bytes);
  }

  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern IntPtr ailu_engine_version();
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_validate_graph_json(string definitionJson);
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_compile_graph_yaml_json(string yaml);
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_available_providers_json();
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_resolve_model_json(string tier, string availableJson, string overrideJson);
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_list_components_json();
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_list_prebuilt_json();
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_run_component_json(string kind, string paramsJson, string channelsJson);
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_run_prebuilt_json(string name, string inputJson, string optionsJson);
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_engine_run_json(string specJson, AiluCallbacks callbacks);
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_engine_resume_json(string specJson, AiluCallbacks callbacks);
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_engine_approve_and_resume_json(string specJson, AiluCallbacks callbacks);
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_engine_signal_json(string specJson, string signalName, string payloadJson, AiluCallbacks callbacks);
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern AiluResult ailu_engine_replay_json(string specJson, string checkpointId, AiluCallbacks callbacks);
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern void ailu_string_free(IntPtr ptr);
  [DllImport("$NativeLibrary", CallingConvention = CallingConvention.Cdecl)]
  public static extern void ailu_result_free(AiluResult result);
}
"@

if (-not ("AiluNative" -as [type])) {
  Add-Type -TypeDefinition $Source
}

function ConvertFrom-AiluResult {
  param([AiluNative+AiluResult] $Result)
  try {
    if ($Result.Code -eq 0) {
      return [AiluNative]::PtrToUtf8($Result.Value)
    }
    $message = if ($Result.Error -eq [IntPtr]::Zero) {
      "Ailu C API error $($Result.Code)"
    } else {
      [AiluNative]::PtrToUtf8($Result.Error)
    }
    throw $message
  } finally {
    [AiluNative]::ailu_result_free($Result)
  }
}

function Get-AiluEngineVersion {
  $ptr = [AiluNative]::ailu_engine_version()
  try {
    [AiluNative]::PtrToUtf8($ptr)
  } finally {
    if ($ptr -ne [IntPtr]::Zero) {
      [AiluNative]::ailu_string_free($ptr)
    }
  }
}

function Test-AiluGraphJson {
  param([string] $DefinitionJson)
  ConvertFrom-AiluResult ([AiluNative]::ailu_validate_graph_json($DefinitionJson))
}

function ConvertFrom-AiluGraphYaml {
  param([string] $Yaml)
  ConvertFrom-AiluResult ([AiluNative]::ailu_compile_graph_yaml_json($Yaml))
}

function Get-AiluAvailableProvidersJson {
  ConvertFrom-AiluResult ([AiluNative]::ailu_available_providers_json())
}

function Resolve-AiluModelJson {
  param([string] $Tier, [string] $AvailableJson = $null, [string] $OverrideJson = $null)
  ConvertFrom-AiluResult ([AiluNative]::ailu_resolve_model_json($Tier, $AvailableJson, $OverrideJson))
}

function Get-AiluComponentsJson {
  ConvertFrom-AiluResult ([AiluNative]::ailu_list_components_json())
}

function Get-AiluPrebuiltJson {
  ConvertFrom-AiluResult ([AiluNative]::ailu_list_prebuilt_json())
}

function Invoke-AiluComponentJson {
  param([string] $Kind, [string] $ParamsJson, [string] $ChannelsJson)
  ConvertFrom-AiluResult ([AiluNative]::ailu_run_component_json($Kind, $ParamsJson, $ChannelsJson))
}

function Invoke-AiluPrebuiltJson {
  param([string] $Name, [string] $InputJson, [string] $OptionsJson = $null)
  ConvertFrom-AiluResult ([AiluNative]::ailu_run_prebuilt_json($Name, $InputJson, $OptionsJson))
}

function Invoke-AiluEngineRunJson {
  param([string] $SpecJson, [AiluNative+AiluCallbacks] $Callbacks)
  ConvertFrom-AiluResult ([AiluNative]::ailu_engine_run_json($SpecJson, $Callbacks))
}

function Invoke-AiluEngineResumeJson {
  param([string] $SpecJson, [AiluNative+AiluCallbacks] $Callbacks)
  ConvertFrom-AiluResult ([AiluNative]::ailu_engine_resume_json($SpecJson, $Callbacks))
}

function Invoke-AiluEngineApproveAndResumeJson {
  param([string] $SpecJson, [AiluNative+AiluCallbacks] $Callbacks)
  ConvertFrom-AiluResult ([AiluNative]::ailu_engine_approve_and_resume_json($SpecJson, $Callbacks))
}

function Invoke-AiluEngineSignalJson {
  param([string] $SpecJson, [string] $SignalName, [string] $PayloadJson, [AiluNative+AiluCallbacks] $Callbacks)
  ConvertFrom-AiluResult ([AiluNative]::ailu_engine_signal_json($SpecJson, $SignalName, $PayloadJson, $Callbacks))
}

function Invoke-AiluEngineReplayJson {
  param([string] $SpecJson, [string] $CheckpointId, [AiluNative+AiluCallbacks] $Callbacks)
  ConvertFrom-AiluResult ([AiluNative]::ailu_engine_replay_json($SpecJson, $CheckpointId, $Callbacks))
}

Export-ModuleMember -Function `
  Get-AiluEngineVersion, `
  Test-AiluGraphJson, `
  ConvertFrom-AiluGraphYaml, `
  Get-AiluAvailableProvidersJson, `
  Resolve-AiluModelJson, `
  Get-AiluComponentsJson, `
  Get-AiluPrebuiltJson, `
  Invoke-AiluComponentJson, `
  Invoke-AiluPrebuiltJson, `
  Invoke-AiluEngineRunJson, `
  Invoke-AiluEngineResumeJson, `
  Invoke-AiluEngineApproveAndResumeJson, `
  Invoke-AiluEngineSignalJson, `
  Invoke-AiluEngineReplayJson
