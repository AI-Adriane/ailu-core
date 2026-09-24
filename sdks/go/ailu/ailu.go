package ailu

/*
#cgo CFLAGS: -I${SRCDIR}/../../../crates/c-api/include
#cgo darwin LDFLAGS: -L${SRCDIR}/../../../crates/target/debug -lailu_c_api
#cgo linux LDFLAGS: -L${SRCDIR}/../../../crates/target/debug -lailu_c_api
#include <stdlib.h>
#include "ailu.h"
*/
import "C"

import (
	"fmt"
	"unsafe"
)

type Error struct {
	Code    int
	Message string
}

func (e Error) Error() string {
	return e.Message
}

func EngineVersion() string {
	ptr := C.ailu_engine_version()
	if ptr == nil {
		return ""
	}
	defer C.ailu_string_free(ptr)
	return C.GoString(ptr)
}

func ValidateGraphJSON(definitionJSON string) (string, error) {
	input := cString(definitionJSON)
	defer C.free(unsafe.Pointer(input))
	return unwrap(C.ailu_validate_graph_json(input))
}

func CompileGraphYAMLJSON(yaml string) (string, error) {
	input := cString(yaml)
	defer C.free(unsafe.Pointer(input))
	return unwrap(C.ailu_compile_graph_yaml_json(input))
}

func AvailableProvidersJSON() (string, error) {
	return unwrap(C.ailu_available_providers_json())
}

func ResolveModelJSON(tier string, availableJSON *string, overrideJSON *string) (string, error) {
	tierC := cString(tier)
	defer C.free(unsafe.Pointer(tierC))
	availableC, freeAvailable := optionalCString(availableJSON)
	defer freeAvailable()
	overrideC, freeOverride := optionalCString(overrideJSON)
	defer freeOverride()
	return unwrap(C.ailu_resolve_model_json(tierC, availableC, overrideC))
}

func ListComponentsJSON() (string, error) {
	return unwrap(C.ailu_list_components_json())
}

func ListPrebuiltJSON() (string, error) {
	return unwrap(C.ailu_list_prebuilt_json())
}

func RunComponentJSON(kind string, paramsJSON string, channelsJSON string) (string, error) {
	kindC := cString(kind)
	paramsC := cString(paramsJSON)
	channelsC := cString(channelsJSON)
	defer C.free(unsafe.Pointer(kindC))
	defer C.free(unsafe.Pointer(paramsC))
	defer C.free(unsafe.Pointer(channelsC))
	return unwrap(C.ailu_run_component_json(kindC, paramsC, channelsC))
}

func RunPrebuiltJSON(name string, inputJSON string, optionsJSON *string) (string, error) {
	nameC := cString(name)
	inputC := cString(inputJSON)
	optionsC, freeOptions := optionalCString(optionsJSON)
	defer C.free(unsafe.Pointer(nameC))
	defer C.free(unsafe.Pointer(inputC))
	defer freeOptions()
	return unwrap(C.ailu_run_prebuilt_json(nameC, inputC, optionsC))
}

func unwrap(result C.AiluResult) (string, error) {
	defer C.ailu_result_free(result)
	if result.code == 0 {
		return C.GoString(result.value), nil
	}

	message := fmt.Sprintf("Ailu C API error %d", int(result.code))
	if result.error != nil {
		message = C.GoString(result.error)
	}
	return "", Error{Code: int(result.code), Message: message}
}

func cString(value string) *C.char {
	return C.CString(value)
}

func optionalCString(value *string) (*C.char, func()) {
	if value == nil {
		return nil, func() {}
	}
	ptr := C.CString(*value)
	return ptr, func() { C.free(unsafe.Pointer(ptr)) }
}
