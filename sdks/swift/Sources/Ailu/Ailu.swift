import Foundation
import CAilu

public struct AiluError: Error, CustomStringConvertible {
    public let code: Int32
    public let message: String

    public var description: String { message }
}

public enum Ailu {
    public static func engineVersion() -> String {
        guard let ptr = ailu_engine_version() else { return "" }
        defer { ailu_string_free(ptr) }
        return String(cString: ptr)
    }

    public static func validateGraphJson(_ definitionJson: String) throws -> String {
        let result = definitionJson.withCString { ailu_validate_graph_json($0) }
        return try unwrap(result)
    }

    public static func compileGraphYamlJson(_ yaml: String) throws -> String {
        let result = yaml.withCString { ailu_compile_graph_yaml_json($0) }
        return try unwrap(result)
    }

    public static func availableProvidersJson() throws -> String {
        try unwrap(ailu_available_providers_json())
    }

    public static func resolveModelJson(_ tier: String, availableJson: String? = nil, overrideJson: String? = nil) throws -> String {
        let result = tier.withCString { tierPtr in
            withOptionalCString(availableJson) { availablePtr in
                withOptionalCString(overrideJson) { overridePtr in
                    ailu_resolve_model_json(tierPtr, availablePtr, overridePtr)
                }
            }
        }
        return try unwrap(result)
    }

    public static func listComponentsJson() throws -> String {
        try unwrap(ailu_list_components_json())
    }

    public static func listPrebuiltJson() throws -> String {
        try unwrap(ailu_list_prebuilt_json())
    }

    public static func runComponentJson(kind: String, paramsJson: String, channelsJson: String) throws -> String {
        let result = kind.withCString { kindPtr in
            paramsJson.withCString { paramsPtr in
                channelsJson.withCString { channelsPtr in
                    ailu_run_component_json(kindPtr, paramsPtr, channelsPtr)
                }
            }
        }
        return try unwrap(result)
    }

    public static func runPrebuiltJson(name: String, inputJson: String, optionsJson: String? = nil) throws -> String {
        let result = name.withCString { namePtr in
            inputJson.withCString { inputPtr in
                withOptionalCString(optionsJson) { optionsPtr in
                    ailu_run_prebuilt_json(namePtr, inputPtr, optionsPtr)
                }
            }
        }
        return try unwrap(result)
    }

    public static func engineRunJson(specJson: String, callbacks: AiluCallbacks) throws -> String {
        let result = specJson.withCString { ailu_engine_run_json($0, callbacks) }
        return try unwrap(result)
    }

    public static func engineResumeJson(specJson: String, callbacks: AiluCallbacks) throws -> String {
        let result = specJson.withCString { ailu_engine_resume_json($0, callbacks) }
        return try unwrap(result)
    }

    public static func engineApproveAndResumeJson(specJson: String, callbacks: AiluCallbacks) throws -> String {
        let result = specJson.withCString { ailu_engine_approve_and_resume_json($0, callbacks) }
        return try unwrap(result)
    }

    public static func engineSignalJson(specJson: String, signalName: String, payloadJson: String, callbacks: AiluCallbacks) throws -> String {
        let result = specJson.withCString { specPtr in
            signalName.withCString { signalPtr in
                payloadJson.withCString { payloadPtr in
                    ailu_engine_signal_json(specPtr, signalPtr, payloadPtr, callbacks)
                }
            }
        }
        return try unwrap(result)
    }

    public static func engineReplayJson(specJson: String, checkpointId: String, callbacks: AiluCallbacks) throws -> String {
        let result = specJson.withCString { specPtr in
            checkpointId.withCString { checkpointPtr in
                ailu_engine_replay_json(specPtr, checkpointPtr, callbacks)
            }
        }
        return try unwrap(result)
    }

    private static func unwrap(_ result: AiluResult) throws -> String {
        defer { ailu_result_free(result) }
        if result.code == 0 {
            guard let value = result.value else { return "" }
            return String(cString: value)
        }

        let message = result.error.map { String(cString: $0) } ?? "Ailu C API error \(result.code)"
        throw AiluError(code: result.code, message: message)
    }

    private static func withOptionalCString<T>(_ value: String?, _ body: (UnsafePointer<CChar>?) throws -> T) rethrows -> T {
        guard let value else {
            return try body(nil)
        }
        return try value.withCString(body)
    }
}
