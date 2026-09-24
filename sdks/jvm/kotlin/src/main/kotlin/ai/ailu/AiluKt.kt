package ai.ailu

object AiluKt {
    fun engineVersion(): String = Ailu.engineVersion()
    fun validateGraphJson(definitionJson: String): String = Ailu.validateGraphJson(definitionJson)
    fun compileGraphYamlJson(yaml: String): String = Ailu.compileGraphYamlJson(yaml)
    fun availableProvidersJson(): String = Ailu.availableProvidersJson()
    fun resolveModelJson(tier: String, availableJson: String? = null, overrideJson: String? = null): String =
        Ailu.resolveModelJson(tier, availableJson, overrideJson)
    fun listComponentsJson(): String = Ailu.listComponentsJson()
    fun listPrebuiltJson(): String = Ailu.listPrebuiltJson()
    fun runComponentJson(kind: String, paramsJson: String, channelsJson: String): String =
        Ailu.runComponentJson(kind, paramsJson, channelsJson)
    fun runPrebuiltJson(name: String, inputJson: String, optionsJson: String? = null): String =
        Ailu.runPrebuiltJson(name, inputJson, optionsJson)
    fun engineRunJson(specJson: String, callbacks: Ailu.AiluCallbacks): String =
        Ailu.engineRunJson(specJson, callbacks)
    fun engineResumeJson(specJson: String, callbacks: Ailu.AiluCallbacks): String =
        Ailu.engineResumeJson(specJson, callbacks)
    fun engineApproveAndResumeJson(specJson: String, callbacks: Ailu.AiluCallbacks): String =
        Ailu.engineApproveAndResumeJson(specJson, callbacks)
    fun engineSignalJson(specJson: String, signalName: String, payloadJson: String, callbacks: Ailu.AiluCallbacks): String =
        Ailu.engineSignalJson(specJson, signalName, payloadJson, callbacks)
    fun engineReplayJson(specJson: String, checkpointId: String, callbacks: Ailu.AiluCallbacks): String =
        Ailu.engineReplayJson(specJson, checkpointId, callbacks)
}
