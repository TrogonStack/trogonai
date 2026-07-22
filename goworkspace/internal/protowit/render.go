package protowit

import "strings"

func render(schema witSchema) string {
	var output strings.Builder
	output.WriteString("package ")
	output.WriteString(schema.packageID.String())
	output.WriteString(";\n\ninterface messages {\n")
	for recordIndex, record := range schema.records {
		if recordIndex > 0 {
			output.WriteByte('\n')
		}
		output.WriteString("  record ")
		output.WriteString(string(record.name))
		output.WriteString(" {\n")
		for _, field := range record.fields {
			output.WriteString("    ")
			output.WriteString(string(field.name))
			output.WriteString(": ")
			output.WriteString(string(field.typeID))
			output.WriteString(",\n")
		}
		output.WriteString("  }\n")
	}
	for variantIndex, variant := range schema.variants {
		if len(schema.records) > 0 || variantIndex > 0 {
			output.WriteByte('\n')
		}
		output.WriteString("  variant ")
		output.WriteString(string(variant.name))
		output.WriteString(" {\n")
		for _, variantCase := range variant.cases {
			output.WriteString("    ")
			output.WriteString(string(variantCase.name))
			if variantCase.typeID != "" {
				output.WriteByte('(')
				output.WriteString(string(variantCase.typeID))
				output.WriteByte(')')
			}
			output.WriteString(",\n")
		}
		output.WriteString("  }\n")
	}
	output.WriteString("}\n")
	return output.String()
}
