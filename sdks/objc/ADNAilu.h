#import <Foundation/Foundation.h>
#import "../../crates/c-api/include/ailu.h"

NS_ASSUME_NONNULL_BEGIN

@interface ADNAilu : NSObject

+ (NSString *)engineVersion;
+ (nullable NSString *)validateGraphJSON:(NSString *)definitionJSON error:(NSError **)error;
+ (nullable NSString *)compileGraphYAMLJSON:(NSString *)yaml error:(NSError **)error;
+ (nullable NSString *)availableProvidersJSON:(NSError **)error;
+ (nullable NSString *)resolveModelJSONWithTier:(NSString *)tier
                                  availableJSON:(nullable NSString *)availableJSON
                                   overrideJSON:(nullable NSString *)overrideJSON
                                          error:(NSError **)error;
+ (nullable NSString *)listComponentsJSON:(NSError **)error;
+ (nullable NSString *)listPrebuiltJSON:(NSError **)error;
+ (nullable NSString *)runComponentJSONWithKind:(NSString *)kind
                                     paramsJSON:(NSString *)paramsJSON
                                   channelsJSON:(NSString *)channelsJSON
                                          error:(NSError **)error;
+ (nullable NSString *)runPrebuiltJSONWithName:(NSString *)name
                                     inputJSON:(NSString *)inputJSON
                                   optionsJSON:(nullable NSString *)optionsJSON
                                         error:(NSError **)error;
+ (nullable NSString *)engineRunJSON:(NSString *)specJSON callbacks:(AiluCallbacks)callbacks error:(NSError **)error;
+ (nullable NSString *)engineResumeJSON:(NSString *)specJSON callbacks:(AiluCallbacks)callbacks error:(NSError **)error;
+ (nullable NSString *)engineApproveAndResumeJSON:(NSString *)specJSON callbacks:(AiluCallbacks)callbacks error:(NSError **)error;
+ (nullable NSString *)engineSignalJSON:(NSString *)specJSON
                             signalName:(NSString *)signalName
                            payloadJSON:(NSString *)payloadJSON
                              callbacks:(AiluCallbacks)callbacks
                                   error:(NSError **)error;
+ (nullable NSString *)engineReplayJSON:(NSString *)specJSON
                           checkpointID:(NSString *)checkpointID
                              callbacks:(AiluCallbacks)callbacks
                                   error:(NSError **)error;

@end

NS_ASSUME_NONNULL_END
