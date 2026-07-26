package protowit

import (
	"fmt"

	"google.golang.org/protobuf/reflect/protoreflect"
)

type unsupportedReason uint8

const (
	unsupportedParameter unsupportedReason = iota + 1
	unsupportedPackage
	unsupportedEnum
	invalidOneofShape
	unsupportedMap
	unsupportedFieldKind
	unsupportedMessageReference
	unsupportedRecursiveMessage
	unsupportedExtension
	unsupportedService
	unsupportedIdentifier
	unsupportedNameCollision
	unsupportedEmptyMessage
)

type schemaError struct {
	element protoreflect.FullName
	reason  unsupportedReason
	detail  string
}

func (e *schemaError) Error() string {
	location := string(e.element)
	if location == "" {
		location = "request"
	}
	if e.detail == "" {
		return fmt.Sprintf("%s: %s", location, e.reason)
	}
	return fmt.Sprintf("%s: %s: %s", location, e.reason, e.detail)
}

func (r unsupportedReason) String() string {
	switch r {
	case unsupportedParameter:
		return "plugin parameters are not supported"
	case unsupportedPackage:
		return "protobuf package cannot be represented in WIT"
	case unsupportedEnum:
		return "enums are not supported"
	case invalidOneofShape:
		return "oneof shape is invalid"
	case unsupportedMap:
		return "maps are not supported"
	case unsupportedFieldKind:
		return "field kind is not supported"
	case unsupportedMessageReference:
		return "message reference is not part of the generated WIT package"
	case unsupportedRecursiveMessage:
		return "recursive message value types are not supported"
	case unsupportedExtension:
		return "extensions are not supported"
	case unsupportedService:
		return "services are not supported"
	case unsupportedIdentifier:
		return "protobuf identifier cannot be represented in WIT"
	case unsupportedNameCollision:
		return "normalized WIT name collision"
	case unsupportedEmptyMessage:
		return "messages without fields can only be represented as oneof members"
	default:
		return "unsupported schema"
	}
}
