#import "ADNAilu.h"
#import "../../crates/c-api/include/ailu.h"

static NSString * const ADNAiluErrorDomain = @"ai.ailu";

@implementation ADNAilu

+ (NSString *)engineVersion {
  char *ptr = ailu_engine_version();
  if (ptr == NULL) {
    return @"";
  }
  NSString *value = [NSString stringWithUTF8String:ptr] ?: @"";
  ailu_string_free(ptr);
  return value;
}

+ (nullable NSString *)validateGraphJSON:(NSString *)definitionJSON error:(NSError **)error {
  return [self unwrap:ailu_validate_graph_json(definitionJSON.UTF8String) error:error];
}

+ (nullable NSString *)compileGraphYAMLJSON:(NSString *)yaml error:(NSError **)error {
  return [self unwrap:ailu_compile_graph_yaml_json(yaml.UTF8String) error:error];
}

+ (nullable NSString *)availableProvidersJSON:(NSError **)error {
  return [self unwrap:ailu_available_providers_json() error:error];
}

+ (nullable NSString *)resolveModelJSONWithTier:(NSString *)tier
                                  availableJSON:(nullable NSString *)availableJSON
                                   overrideJSON:(nullable NSString *)overrideJSON
                                          error:(NSError **)error {
  return [self unwrap:ailu_resolve_model_json(tier.UTF8String, availableJSON.UTF8String, overrideJSON.UTF8String) error:error];
}

+ (nullable NSString *)listComponentsJSON:(NSError **)error {
  return [self unwrap:ailu_list_components_json() error:error];
}

+ (nullable NSString *)listPrebuiltJSON:(NSError **)error {
  return [self unwrap:ailu_list_prebuilt_json() error:error];
}

+ (nullable NSString *)runComponentJSONWithKind:(NSString *)kind
                                     paramsJSON:(NSString *)paramsJSON
                                   channelsJSON:(NSString *)channelsJSON
                                          error:(NSError **)error {
  return [self unwrap:ailu_run_component_json(kind.UTF8String, paramsJSON.UTF8String, channelsJSON.UTF8String) error:error];
}

+ (nullable NSString *)runPrebuiltJSONWithName:(NSString *)name
                                     inputJSON:(NSString *)inputJSON
                                   optionsJSON:(nullable NSString *)optionsJSON
                                         error:(NSError **)error {
  return [self unwrap:ailu_run_prebuilt_json(name.UTF8String, inputJSON.UTF8String, optionsJSON.UTF8String) error:error];
}

+ (nullable NSString *)engineRunJSON:(NSString *)specJSON callbacks:(AiluCallbacks)callbacks error:(NSError **)error {
  return [self unwrap:ailu_engine_run_json(specJSON.UTF8String, callbacks) error:error];
}

+ (nullable NSString *)engineResumeJSON:(NSString *)specJSON callbacks:(AiluCallbacks)callbacks error:(NSError **)error {
  return [self unwrap:ailu_engine_resume_json(specJSON.UTF8String, callbacks) error:error];
}

+ (nullable NSString *)engineApproveAndResumeJSON:(NSString *)specJSON callbacks:(AiluCallbacks)callbacks error:(NSError **)error {
  return [self unwrap:ailu_engine_approve_and_resume_json(specJSON.UTF8String, callbacks) error:error];
}

+ (nullable NSString *)engineSignalJSON:(NSString *)specJSON
                             signalName:(NSString *)signalName
                            payloadJSON:(NSString *)payloadJSON
                              callbacks:(AiluCallbacks)callbacks
                                   error:(NSError **)error {
  return [self unwrap:ailu_engine_signal_json(specJSON.UTF8String, signalName.UTF8String, payloadJSON.UTF8String, callbacks) error:error];
}

+ (nullable NSString *)engineReplayJSON:(NSString *)specJSON
                           checkpointID:(NSString *)checkpointID
                              callbacks:(AiluCallbacks)callbacks
                                   error:(NSError **)error {
  return [self unwrap:ailu_engine_replay_json(specJSON.UTF8String, checkpointID.UTF8String, callbacks) error:error];
}

+ (nullable NSString *)unwrap:(AiluResult)result error:(NSError **)error {
  if (result.code == AILU_OK) {
    NSString *value = result.value == NULL ? @"" : [NSString stringWithUTF8String:result.value];
    ailu_result_free(result);
    return value ?: @"";
  }

  NSString *message = result.error == NULL
      ? [NSString stringWithFormat:@"Ailu C API error %d", result.code]
      : [NSString stringWithUTF8String:result.error];
  if (error != nil) {
    *error = [NSError errorWithDomain:ADNAiluErrorDomain
                                 code:result.code
                             userInfo:@{NSLocalizedDescriptionKey: message ?: @"Ailu C API error"}];
  }
  ailu_result_free(result);
  return nil;
}

@end
