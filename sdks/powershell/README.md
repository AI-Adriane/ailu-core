# Ailu PowerShell SDK

PowerShell module over `ailu-c-api` using an embedded C# P/Invoke bridge.

```powershell
Import-Module ./Ailu.psm1
Get-AiluEngineVersion
Get-AiluComponentsJson
```

The native `ailu_c_api` library must be on the platform loader path.
