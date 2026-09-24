package ai.ailu

object AiluScala {
  def engineVersion(): String = Ailu.engineVersion()
  def validateGraphJson(definitionJson: String): String = Ailu.validateGraphJson(definitionJson)
  def compileGraphYamlJson(yaml: String): String = Ailu.compileGraphYamlJson(yaml)
  def availableProvidersJson(): String = Ailu.availableProvidersJson()
  def resolveModelJson(tier: String, availableJson: String = null, overrideJson: String = null): String =
    Ailu.resolveModelJson(tier, availableJson, overrideJson)
  def listComponentsJson(): String = Ailu.listComponentsJson()
  def listPrebuiltJson(): String = Ailu.listPrebuiltJson()
  def runComponentJson(kind: String, paramsJson: String, channelsJson: String): String =
    Ailu.runComponentJson(kind, paramsJson, channelsJson)
  def runPrebuiltJson(name: String, inputJson: String, optionsJson: String = null): String =
    Ailu.runPrebuiltJson(name, inputJson, optionsJson)
  def engineRunJson(specJson: String, callbacks: Ailu.AiluCallbacks): String =
    Ailu.engineRunJson(specJson, callbacks)
  def engineResumeJson(specJson: String, callbacks: Ailu.AiluCallbacks): String =
    Ailu.engineResumeJson(specJson, callbacks)
  def engineApproveAndResumeJson(specJson: String, callbacks: Ailu.AiluCallbacks): String =
    Ailu.engineApproveAndResumeJson(specJson, callbacks)
  def engineSignalJson(specJson: String, signalName: String, payloadJson: String, callbacks: Ailu.AiluCallbacks): String =
    Ailu.engineSignalJson(specJson, signalName, payloadJson, callbacks)
  def engineReplayJson(specJson: String, checkpointId: String, callbacks: Ailu.AiluCallbacks): String =
    Ailu.engineReplayJson(specJson, checkpointId, callbacks)
}
