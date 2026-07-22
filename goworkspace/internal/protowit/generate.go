package protowit

import (
	"fmt"
	"sort"
	"strings"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protodesc"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/types/descriptorpb"
	"google.golang.org/protobuf/types/pluginpb"
)

type witType string

type witField struct {
	name           witIdentifier
	typeID         witType
	messageTargets []protoreflect.FullName
}

type witRecord struct {
	fullName protoreflect.FullName
	name     witIdentifier
	fields   []witField
}

type witVariantCase struct {
	name          witIdentifier
	typeID        witType
	messageTarget protoreflect.FullName
}

type witVariant struct {
	fullName protoreflect.FullName
	name     witIdentifier
	cases    []witVariantCase
}

type witSchema struct {
	packageID    witPackageID
	path         string
	protoPackage protoreflect.FullName
	records      []witRecord
	variants     []witVariant
}

type messageDefinition struct {
	descriptor protoreflect.MessageDescriptor
	name       witIdentifier
}

type oneofDefinition struct {
	descriptor protoreflect.OneofDescriptor
	name       witIdentifier
}

func generateFiles(request *pluginpb.CodeGeneratorRequest) ([]*pluginpb.CodeGeneratorResponse_File, error) {
	if request.GetParameter() != "" {
		return nil, &schemaError{reason: unsupportedParameter, detail: request.GetParameter()}
	}

	registry, err := protodesc.NewFiles(&descriptorpb.FileDescriptorSet{File: request.GetProtoFile()})
	if err != nil {
		return nil, fmt.Errorf("link protobuf descriptors: %w", err)
	}

	requestedPaths := append([]string(nil), request.GetFileToGenerate()...)
	sort.Strings(requestedPaths)
	filesByPackage := make(map[protoreflect.FullName][]protoreflect.FileDescriptor)
	seenPaths := make(map[string]struct{}, len(requestedPaths))
	for _, requestedPath := range requestedPaths {
		if _, found := seenPaths[requestedPath]; found {
			continue
		}
		seenPaths[requestedPath] = struct{}{}
		file, err := registry.FindFileByPath(requestedPath)
		if err != nil {
			return nil, fmt.Errorf("find protobuf file %q: %w", requestedPath, err)
		}
		filesByPackage[file.Package()] = append(filesByPackage[file.Package()], file)
	}

	protoPackages := make([]protoreflect.FullName, 0, len(filesByPackage))
	for protoPackage := range filesByPackage {
		protoPackages = append(protoPackages, protoPackage)
	}
	sort.Slice(protoPackages, func(left, right int) bool {
		return protoPackages[left] < protoPackages[right]
	})

	schemas := make([]witSchema, 0, len(protoPackages))
	packageOwners := make(map[string]protoreflect.FullName)
	pathOwners := make(map[string]protoreflect.FullName)
	for _, protoPackage := range protoPackages {
		schema, err := buildSchema(filesByPackage[protoPackage])
		if err != nil {
			return nil, err
		}
		packageName := schema.packageID.String()
		if owner, found := packageOwners[packageName]; found && owner != schema.protoPackage {
			return nil, &schemaError{
				element: schema.protoPackage,
				reason:  unsupportedNameCollision,
				detail:  string(owner),
			}
		}
		packageOwners[packageName] = schema.protoPackage
		if owner, found := pathOwners[schema.path]; found && owner != schema.protoPackage {
			return nil, &schemaError{
				element: schema.protoPackage,
				reason:  unsupportedNameCollision,
				detail:  string(owner),
			}
		}
		pathOwners[schema.path] = schema.protoPackage
		schemas = append(schemas, schema)
	}

	sort.Slice(schemas, func(left, right int) bool {
		if schemas[left].path == schemas[right].path {
			return schemas[left].protoPackage < schemas[right].protoPackage
		}
		return schemas[left].path < schemas[right].path
	})

	responses := make([]*pluginpb.CodeGeneratorResponse_File, 0, len(schemas))
	for _, schema := range schemas {
		content := render(schema)
		responses = append(responses, &pluginpb.CodeGeneratorResponse_File{
			Name:    proto.String(schema.path),
			Content: proto.String(content),
		})
	}
	return responses, nil
}

func buildSchema(files []protoreflect.FileDescriptor) (witSchema, error) {
	if len(files) == 0 {
		return witSchema{}, fmt.Errorf("build WIT schema: no protobuf files")
	}

	orderedFiles := append([]protoreflect.FileDescriptor(nil), files...)
	sort.Slice(orderedFiles, func(left, right int) bool {
		return orderedFiles[left].Path() < orderedFiles[right].Path()
	})

	protoPackage := orderedFiles[0].Package()
	protobufPackage := string(protoPackage)
	id, err := packageID(protobufPackage)
	if err != nil {
		return witSchema{}, err
	}

	definitions := make([]messageDefinition, 0)
	namesByIdentifier := make(map[witIdentifier]protoreflect.FullName)
	namesByFullName := make(map[protoreflect.FullName]witIdentifier)
	requestedMessages := make([]protoreflect.MessageDescriptor, 0)
	for _, file := range orderedFiles {
		if file.Package() != protoPackage {
			return witSchema{}, fmt.Errorf(
				"build WIT schema for protobuf package %q: file %q belongs to %q",
				protoPackage,
				file.Path(),
				file.Package(),
			)
		}
		if file.Services().Len() > 0 {
			return witSchema{}, &schemaError{element: file.Package(), reason: unsupportedService}
		}
		if file.Extensions().Len() > 0 {
			return witSchema{}, &schemaError{element: file.Package(), reason: unsupportedExtension}
		}
		if file.Enums().Len() > 0 {
			return witSchema{}, &schemaError{element: file.Enums().Get(0).FullName(), reason: unsupportedEnum}
		}
		for index := 0; index < file.Messages().Len(); index++ {
			requestedMessages = append(requestedMessages, file.Messages().Get(index))
		}
	}
	sort.Slice(requestedMessages, func(left, right int) bool {
		return requestedMessages[left].FullName() < requestedMessages[right].FullName()
	})
	for _, message := range requestedMessages {
		if err := collectMessageDefinitions(
			message,
			protoPackage,
			&definitions,
			namesByIdentifier,
			namesByFullName,
		); err != nil {
			return witSchema{}, err
		}
	}
	if err := collectSamePackageReferences(
		protoPackage,
		&definitions,
		namesByIdentifier,
		namesByFullName,
	); err != nil {
		return witSchema{}, err
	}

	sort.Slice(definitions, func(left, right int) bool {
		return definitions[left].descriptor.FullName() < definitions[right].descriptor.FullName()
	})
	oneofDefinitions, err := collectOneofDefinitions(definitions, namesByIdentifier)
	if err != nil {
		return witSchema{}, err
	}
	variants := make([]witVariant, 0, len(oneofDefinitions))
	variantsByFullName := make(map[protoreflect.FullName]witVariant, len(oneofDefinitions))
	for _, definition := range oneofDefinitions {
		variant, err := buildVariant(definition, namesByFullName)
		if err != nil {
			return witSchema{}, err
		}
		variants = append(variants, variant)
		variantsByFullName[variant.fullName] = variant
	}
	emptyOneofMembers := emptyOneofMemberNames(oneofDefinitions)
	records := make([]witRecord, 0, len(definitions))
	for _, definition := range definitions {
		if isEmptyMessage(definition.descriptor) {
			if _, found := emptyOneofMembers[definition.descriptor.FullName()]; found {
				continue
			}
			return witSchema{}, &schemaError{
				element: definition.descriptor.FullName(),
				reason:  unsupportedEmptyMessage,
			}
		}
		record, err := buildRecord(definition, namesByFullName, variantsByFullName)
		if err != nil {
			return witSchema{}, err
		}
		records = append(records, record)
	}
	if err := detectMessageCycles(records); err != nil {
		return witSchema{}, err
	}

	return witSchema{
		packageID:    id,
		path:         id.outputDirectory() + "/messages.wit",
		protoPackage: protoPackage,
		records:      records,
		variants:     variants,
	}, nil
}

func collectMessageDefinitions(
	message protoreflect.MessageDescriptor,
	protoPackage protoreflect.FullName,
	definitions *[]messageDefinition,
	namesByIdentifier map[witIdentifier]protoreflect.FullName,
	namesByFullName map[protoreflect.FullName]witIdentifier,
) error {
	if message.IsMapEntry() {
		return nil
	}
	if _, found := namesByFullName[message.FullName()]; found {
		return nil
	}
	if message.Enums().Len() > 0 {
		return &schemaError{element: message.Enums().Get(0).FullName(), reason: unsupportedEnum}
	}
	if message.Extensions().Len() > 0 {
		return &schemaError{element: message.FullName(), reason: unsupportedExtension}
	}
	relativeName := strings.TrimPrefix(string(message.FullName()), string(protoPackage)+".")
	if relativeName == string(message.FullName()) {
		return &schemaError{
			element: message.FullName(),
			reason:  unsupportedMessageReference,
			detail:  string(message.FullName()),
		}
	}
	name, err := normalizeIdentifier(relativeName)
	if err != nil {
		return &schemaError{element: message.FullName(), reason: unsupportedIdentifier, detail: err.Error()}
	}
	if existing, found := namesByIdentifier[name]; found {
		return &schemaError{
			element: message.FullName(),
			reason:  unsupportedNameCollision,
			detail:  string(existing),
		}
	}
	namesByIdentifier[name] = message.FullName()
	namesByFullName[message.FullName()] = name
	*definitions = append(*definitions, messageDefinition{descriptor: message, name: name})

	nestedMessages := make([]protoreflect.MessageDescriptor, 0, message.Messages().Len())
	for index := 0; index < message.Messages().Len(); index++ {
		nestedMessages = append(nestedMessages, message.Messages().Get(index))
	}
	sort.Slice(nestedMessages, func(left, right int) bool {
		return nestedMessages[left].FullName() < nestedMessages[right].FullName()
	})
	for _, nestedMessage := range nestedMessages {
		if err := collectMessageDefinitions(
			nestedMessage,
			protoPackage,
			definitions,
			namesByIdentifier,
			namesByFullName,
		); err != nil {
			return err
		}
	}
	return nil
}

func collectOneofDefinitions(
	definitions []messageDefinition,
	namesByIdentifier map[witIdentifier]protoreflect.FullName,
) ([]oneofDefinition, error) {
	oneofs := make([]protoreflect.OneofDescriptor, 0)
	messageNames := make(map[protoreflect.FullName]witIdentifier, len(definitions))
	for _, definition := range definitions {
		messageNames[definition.descriptor.FullName()] = definition.name
		for index := 0; index < definition.descriptor.Oneofs().Len(); index++ {
			oneof := definition.descriptor.Oneofs().Get(index)
			if !oneof.IsSynthetic() {
				oneofs = append(oneofs, oneof)
			}
		}
	}
	sort.Slice(oneofs, func(left, right int) bool {
		return oneofs[left].FullName() < oneofs[right].FullName()
	})

	result := make([]oneofDefinition, 0, len(oneofs))
	for _, oneof := range oneofs {
		messageName := strings.TrimPrefix(string(messageNames[oneof.Parent().FullName()]), "%")
		name, err := normalizeIdentifier(messageName + "." + string(oneof.Name()))
		if err != nil {
			return nil, &schemaError{
				element: oneof.FullName(),
				reason:  unsupportedIdentifier,
				detail:  err.Error(),
			}
		}
		if existing, found := namesByIdentifier[name]; found {
			return nil, &schemaError{
				element: oneof.FullName(),
				reason:  unsupportedNameCollision,
				detail:  string(existing),
			}
		}
		namesByIdentifier[name] = oneof.FullName()
		result = append(result, oneofDefinition{descriptor: oneof, name: name})
	}
	return result, nil
}

func collectSamePackageReferences(
	protoPackage protoreflect.FullName,
	definitions *[]messageDefinition,
	namesByIdentifier map[witIdentifier]protoreflect.FullName,
	namesByFullName map[protoreflect.FullName]witIdentifier,
) error {
	for {
		pending := make(map[protoreflect.FullName]protoreflect.MessageDescriptor)
		for _, definition := range *definitions {
			fields := definition.descriptor.Fields()
			for index := 0; index < fields.Len(); index++ {
				field := fields.Get(index)
				if field.IsMap() || field.Kind() != protoreflect.MessageKind {
					continue
				}
				target := field.Message()
				if target.ParentFile().Package() != protoPackage {
					continue
				}
				if _, found := namesByFullName[target.FullName()]; !found {
					pending[target.FullName()] = target
				}
			}
		}
		if len(pending) == 0 {
			return nil
		}

		fullNames := make([]protoreflect.FullName, 0, len(pending))
		for fullName := range pending {
			fullNames = append(fullNames, fullName)
		}
		sort.Slice(fullNames, func(left, right int) bool {
			return fullNames[left] < fullNames[right]
		})
		for _, fullName := range fullNames {
			if err := collectMessageDefinitions(
				pending[fullName],
				protoPackage,
				definitions,
				namesByIdentifier,
				namesByFullName,
			); err != nil {
				return err
			}
		}
	}
}

func buildRecord(
	definition messageDefinition,
	namesByFullName map[protoreflect.FullName]witIdentifier,
	variantsByFullName map[protoreflect.FullName]witVariant,
) (witRecord, error) {
	message := definition.descriptor

	fields := make([]witField, 0, message.Fields().Len())
	orderedFields := make([]protoreflect.FieldDescriptor, 0, message.Fields().Len())
	for index := 0; index < message.Fields().Len(); index++ {
		orderedFields = append(orderedFields, message.Fields().Get(index))
	}
	sort.Slice(orderedFields, func(left, right int) bool {
		if orderedFields[left].Number() == orderedFields[right].Number() {
			return orderedFields[left].FullName() < orderedFields[right].FullName()
		}
		return orderedFields[left].Number() < orderedFields[right].Number()
	})
	fieldNames := make(map[witIdentifier]protoreflect.FullName)
	seenOneofs := make(map[protoreflect.FullName]struct{})
	for _, field := range orderedFields {
		var converted witField
		var owner protoreflect.FullName
		var err error
		oneof := field.ContainingOneof()
		if oneof != nil && !oneof.IsSynthetic() {
			if _, found := seenOneofs[oneof.FullName()]; found {
				continue
			}
			seenOneofs[oneof.FullName()] = struct{}{}
			variant, found := variantsByFullName[oneof.FullName()]
			if !found {
				return witRecord{}, fmt.Errorf("build WIT record %q: missing oneof variant %q", message.FullName(), oneof.FullName())
			}
			converted, err = buildOneofField(oneof, variant)
			owner = oneof.FullName()
		} else {
			converted, err = buildField(field, namesByFullName)
			owner = field.FullName()
		}
		if err != nil {
			return witRecord{}, err
		}
		if existing, found := fieldNames[converted.name]; found {
			return witRecord{}, &schemaError{
				element: owner,
				reason:  unsupportedNameCollision,
				detail:  string(existing),
			}
		}
		fieldNames[converted.name] = owner
		fields = append(fields, converted)
	}
	return witRecord{fullName: message.FullName(), name: definition.name, fields: fields}, nil
}

func buildOneofField(oneof protoreflect.OneofDescriptor, variant witVariant) (witField, error) {
	name, err := normalizeIdentifier(string(oneof.Name()))
	if err != nil {
		return witField{}, &schemaError{element: oneof.FullName(), reason: unsupportedIdentifier, detail: err.Error()}
	}
	targets := make([]protoreflect.FullName, 0, len(variant.cases))
	for _, variantCase := range variant.cases {
		if variantCase.messageTarget != "" {
			targets = append(targets, variantCase.messageTarget)
		}
	}
	return witField{
		name:           name,
		typeID:         witType("option<" + string(variant.name) + ">"),
		messageTargets: targets,
	}, nil
}

func buildVariant(
	definition oneofDefinition,
	namesByFullName map[protoreflect.FullName]witIdentifier,
) (witVariant, error) {
	oneof := definition.descriptor
	if oneof.Fields().Len() == 0 {
		return witVariant{}, &schemaError{
			element: oneof.FullName(),
			reason:  invalidOneofShape,
			detail:  "oneof must contain at least one member",
		}
	}
	orderedFields := make([]protoreflect.FieldDescriptor, 0, oneof.Fields().Len())
	for index := 0; index < oneof.Fields().Len(); index++ {
		orderedFields = append(orderedFields, oneof.Fields().Get(index))
	}
	sort.Slice(orderedFields, func(left, right int) bool {
		if orderedFields[left].Number() == orderedFields[right].Number() {
			return orderedFields[left].FullName() < orderedFields[right].FullName()
		}
		return orderedFields[left].Number() < orderedFields[right].Number()
	})

	cases := make([]witVariantCase, 0, len(orderedFields))
	caseNames := make(map[witIdentifier]protoreflect.FullName, len(orderedFields))
	for _, field := range orderedFields {
		if field.Cardinality() == protoreflect.Repeated {
			return witVariant{}, &schemaError{
				element: field.FullName(),
				reason:  invalidOneofShape,
				detail:  "oneof members cannot be repeated",
			}
		}
		name, err := normalizeIdentifier(string(field.Name()))
		if err != nil {
			return witVariant{}, &schemaError{element: field.FullName(), reason: unsupportedIdentifier, detail: err.Error()}
		}
		if existing, found := caseNames[name]; found {
			return witVariant{}, &schemaError{
				element: field.FullName(),
				reason:  unsupportedNameCollision,
				detail:  string(existing),
			}
		}
		caseNames[name] = field.FullName()
		if field.Kind() == protoreflect.MessageKind && isEmptyMessage(field.Message()) {
			cases = append(cases, witVariantCase{name: name})
			continue
		}
		typeID, messageTarget, err := baseFieldType(field, namesByFullName)
		if err != nil {
			return witVariant{}, err
		}
		cases = append(cases, witVariantCase{name: name, typeID: typeID, messageTarget: messageTarget})
	}
	return witVariant{fullName: oneof.FullName(), name: definition.name, cases: cases}, nil
}

func buildField(
	field protoreflect.FieldDescriptor,
	namesByFullName map[protoreflect.FullName]witIdentifier,
) (witField, error) {
	if field.IsMap() {
		return witField{}, &schemaError{element: field.FullName(), reason: unsupportedMap}
	}

	name, err := normalizeIdentifier(string(field.Name()))
	if err != nil {
		return witField{}, &schemaError{element: field.FullName(), reason: unsupportedIdentifier, detail: err.Error()}
	}
	if field.Kind() == protoreflect.MessageKind && isEmptyMessage(field.Message()) {
		return witField{}, &schemaError{
			element: field.FullName(),
			reason:  unsupportedEmptyMessage,
			detail:  string(field.Message().FullName()),
		}
	}
	typeID, messageTarget, err := baseFieldType(field, namesByFullName)
	if err != nil {
		return witField{}, err
	}
	if field.Cardinality() == protoreflect.Repeated {
		typeID = witType("list<" + string(typeID) + ">")
	} else if field.HasPresence() && field.Cardinality() != protoreflect.Required {
		typeID = witType("option<" + string(typeID) + ">")
	}
	return witField{
		name:   name,
		typeID: typeID,
		messageTargets: func() []protoreflect.FullName {
			if messageTarget == "" {
				return nil
			}
			return []protoreflect.FullName{messageTarget}
		}(),
	}, nil
}

func isEmptyMessage(message protoreflect.MessageDescriptor) bool {
	return message.Fields().Len() == 0
}

func emptyOneofMemberNames(definitions []oneofDefinition) map[protoreflect.FullName]struct{} {
	names := make(map[protoreflect.FullName]struct{})
	for _, definition := range definitions {
		fields := definition.descriptor.Fields()
		for index := 0; index < fields.Len(); index++ {
			field := fields.Get(index)
			if field.Kind() == protoreflect.MessageKind && isEmptyMessage(field.Message()) {
				names[field.Message().FullName()] = struct{}{}
			}
		}
	}
	return names
}

func baseFieldType(
	field protoreflect.FieldDescriptor,
	namesByFullName map[protoreflect.FullName]witIdentifier,
) (witType, protoreflect.FullName, error) {
	if field.IsMap() {
		return "", "", &schemaError{element: field.FullName(), reason: unsupportedMap}
	}
	if field.Kind() == protoreflect.MessageKind {
		messageTarget := field.Message().FullName()
		messageName, found := namesByFullName[messageTarget]
		if !found {
			return "", "", &schemaError{
				element: field.FullName(),
				reason:  unsupportedMessageReference,
				detail:  string(messageTarget),
			}
		}
		return witType(messageName), messageTarget, nil
	}
	typeID, err := scalarType(field)
	return typeID, "", err
}

func detectMessageCycles(records []witRecord) error {
	dependencies := make(map[protoreflect.FullName][]protoreflect.FullName, len(records))
	for _, record := range records {
		seen := make(map[protoreflect.FullName]struct{})
		for _, field := range record.fields {
			for _, messageTarget := range field.messageTargets {
				if _, found := seen[messageTarget]; found {
					continue
				}
				seen[messageTarget] = struct{}{}
				dependencies[record.fullName] = append(dependencies[record.fullName], messageTarget)
			}
		}
		sort.Slice(dependencies[record.fullName], func(left, right int) bool {
			return dependencies[record.fullName][left] < dependencies[record.fullName][right]
		})
	}

	const (
		visiting uint8 = iota + 1
		visited
	)
	state := make(map[protoreflect.FullName]uint8, len(records))
	stack := make([]protoreflect.FullName, 0, len(records))
	var visit func(protoreflect.FullName) error
	visit = func(name protoreflect.FullName) error {
		state[name] = visiting
		stack = append(stack, name)
		for _, dependency := range dependencies[name] {
			switch state[dependency] {
			case visiting:
				start := 0
				for stack[start] != dependency {
					start++
				}
				cycle := append(append([]protoreflect.FullName(nil), stack[start:]...), dependency)
				parts := make([]string, len(cycle))
				for index, cycleName := range cycle {
					parts[index] = string(cycleName)
				}
				return &schemaError{
					element: dependency,
					reason:  unsupportedRecursiveMessage,
					detail:  strings.Join(parts, " -> "),
				}
			case 0:
				if err := visit(dependency); err != nil {
					return err
				}
			}
		}
		stack = stack[:len(stack)-1]
		state[name] = visited
		return nil
	}

	for _, record := range records {
		if state[record.fullName] == 0 {
			if err := visit(record.fullName); err != nil {
				return err
			}
		}
	}
	return nil
}

func scalarType(field protoreflect.FieldDescriptor) (witType, error) {
	switch field.Kind() {
	case protoreflect.BoolKind:
		return "bool", nil
	case protoreflect.StringKind:
		return "string", nil
	case protoreflect.BytesKind:
		return "list<u8>", nil
	case protoreflect.FloatKind:
		return "f32", nil
	case protoreflect.DoubleKind:
		return "f64", nil
	case protoreflect.Int32Kind, protoreflect.Sint32Kind, protoreflect.Sfixed32Kind:
		return "s32", nil
	case protoreflect.Uint32Kind, protoreflect.Fixed32Kind:
		return "u32", nil
	case protoreflect.Int64Kind, protoreflect.Sint64Kind, protoreflect.Sfixed64Kind:
		return "s64", nil
	case protoreflect.Uint64Kind, protoreflect.Fixed64Kind:
		return "u64", nil
	case protoreflect.EnumKind:
		return "", &schemaError{element: field.FullName(), reason: unsupportedEnum}
	default:
		return "", &schemaError{
			element: field.FullName(),
			reason:  unsupportedFieldKind,
			detail:  field.Kind().String(),
		}
	}
}
