package protowit

import (
	"fmt"
	"strings"
)

type witIdentifier string

type witPackageID struct {
	namespace witIdentifier
	name      witIdentifier
}

func (p witPackageID) String() string {
	return string(p.namespace) + ":" + string(p.name)
}

func (p witPackageID) outputDirectory() string {
	return strings.TrimPrefix(string(p.namespace), "%") + "/" +
		strings.TrimPrefix(string(p.name), "%")
}

func packageID(protobufPackage string) (witPackageID, error) {
	parts := strings.Split(protobufPackage, ".")
	if len(parts) < 2 {
		return witPackageID{}, &schemaError{reason: unsupportedPackage, detail: protobufPackage}
	}
	namespace, err := encodePackageSegment(parts[0])
	if err != nil {
		return witPackageID{}, &schemaError{reason: unsupportedPackage, detail: err.Error()}
	}
	encodedName := make([]string, 0, len(parts)-1)
	for _, part := range parts[1:] {
		encoded, err := encodePackageSegment(part)
		if err != nil {
			return witPackageID{}, &schemaError{reason: unsupportedPackage, detail: err.Error()}
		}
		encodedName = append(encodedName, encoded)
	}
	return witPackageID{
		namespace: witIdentifier(escapeWITKeyword(namespace)),
		name:      witIdentifier(escapeWITKeyword(strings.Join(encodedName, "-"))),
	}, nil
}

func encodePackageSegment(input string) (string, error) {
	if input == "" || isASCIIDigit(input[0]) {
		return "", fmt.Errorf("package segment %q cannot be represented in WIT", input)
	}

	const hexadecimal = "0123456789abcdef"
	var output strings.Builder
	for index := 0; index < len(input); index++ {
		current := input[index]
		switch {
		case isASCIILower(current) && current != 'z', isASCIIDigit(current):
			output.WriteByte(current)
		case current == 'z':
			output.WriteString("zz")
		case isASCIIUpper(current), current == '_':
			output.WriteString("zx")
			output.WriteByte(hexadecimal[current>>4])
			output.WriteByte(hexadecimal[current&0x0f])
		default:
			return "", fmt.Errorf("package segment %q contains unsupported character %q", input, current)
		}
	}
	return output.String(), nil
}

func normalizeIdentifier(input string) (witIdentifier, error) {
	if input == "" {
		return "", fmt.Errorf("identifier is empty")
	}

	var output strings.Builder
	for index := 0; index < len(input); index++ {
		current := input[index]
		if current == '_' || current == '-' || current == '.' {
			if output.Len() > 0 && output.String()[output.Len()-1] != '-' {
				output.WriteByte('-')
			}
			continue
		}
		if !isASCIIAlphaNumeric(current) {
			return "", fmt.Errorf("identifier %q contains unsupported character %q", input, current)
		}
		if isASCIIUpper(current) {
			previousIsLowerOrDigit := index > 0 && (isASCIILower(input[index-1]) || isASCIIDigit(input[index-1]))
			nextIsLower := index+1 < len(input) && isASCIILower(input[index+1])
			if output.Len() > 0 && output.String()[output.Len()-1] != '-' && (previousIsLowerOrDigit || nextIsLower) {
				output.WriteByte('-')
			}
			current += 'a' - 'A'
		}
		output.WriteByte(current)
	}

	normalized := strings.Trim(output.String(), "-")
	if normalized == "" || isASCIIDigit(normalized[0]) {
		return "", fmt.Errorf("identifier %q cannot be represented in WIT", input)
	}
	return witIdentifier(escapeWITKeyword(normalized)), nil
}

func escapeWITKeyword(identifier string) string {
	if _, found := witKeywords[identifier]; found {
		return "%" + identifier
	}
	return identifier
}

func isASCIIAlphaNumeric(value byte) bool {
	return isASCIILower(value) || isASCIIUpper(value) || isASCIIDigit(value)
}

func isASCIILower(value byte) bool {
	return value >= 'a' && value <= 'z'
}

func isASCIIUpper(value byte) bool {
	return value >= 'A' && value <= 'Z'
}

func isASCIIDigit(value byte) bool {
	return value >= '0' && value <= '9'
}

var witKeywords = map[string]struct{}{
	"as": {}, "async": {}, "bool": {}, "borrow": {}, "char": {}, "constructor": {},
	"enum": {}, "export": {}, "f32": {}, "f64": {}, "flags": {}, "from": {},
	"error-context": {},
	"func":          {}, "future": {}, "import": {}, "include": {}, "interface": {}, "list": {},
	"map": {}, "option": {}, "own": {}, "package": {}, "record": {}, "resource": {},
	"result": {}, "s16": {}, "s32": {}, "s64": {}, "s8": {}, "static": {},
	"stream": {}, "string": {}, "tuple": {}, "type": {}, "u16": {}, "u32": {},
	"u64": {}, "u8": {}, "use": {}, "variant": {}, "with": {}, "world": {},
}
