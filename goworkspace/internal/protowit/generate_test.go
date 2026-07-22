package protowit

import (
	"bytes"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/descriptorpb"
	"google.golang.org/protobuf/types/pluginpb"
)

func TestRunGeneratesDeterministicWIT(t *testing.T) {
	requireGeneratedSnapshot(t, "scalars", runRequest(t, scalarRequest()))
}

func TestScalarOneofGeneratesVariantSnapshot(t *testing.T) {
	requireGeneratedSnapshot(t, "scalar-oneof", runRequest(t, scalarOneofRequest()))
}

func TestMessageOneofEmptyPayloadsBecomePayloadlessCases(t *testing.T) {
	requireGeneratedSnapshot(t, "message-oneof", runRequest(t, messageOneofRequest()))
}

func TestMultipleOneofsGenerateDeterministically(t *testing.T) {
	request := multipleOneofsRequest()
	response := runRequest(t, request)
	requireGeneratedSnapshot(t, "multiple-oneofs", response)

	shuffled := proto.Clone(request).(*pluginpb.CodeGeneratorRequest)
	reverse(shuffled.ProtoFile[0].MessageType[0].Field)
	require.True(
		t,
		proto.Equal(response, runRequest(t, shuffled)),
		"oneof output depends on field declaration order",
	)
}

func TestOneofMessageCaseIncludesTransitiveSamePackageImport(t *testing.T) {
	requireGeneratedSnapshot(t, "oneof-import", runRequest(t, importedOneofRequest()))
}

func TestRunRejectsMalformedInput(t *testing.T) {
	var output bytes.Buffer
	err := Run(bytes.NewReader([]byte{0xff}), &output)
	require.ErrorContains(t, err, "decode code generator request")
	require.Empty(t, output.Bytes(), "malformed input must not produce output")
}

func TestResponseAdvertisesEdition2024(t *testing.T) {
	response := runRequest(t, scalarRequest())
	wantFeature := uint64(
		pluginpb.CodeGeneratorResponse_FEATURE_PROTO3_OPTIONAL |
			pluginpb.CodeGeneratorResponse_FEATURE_SUPPORTS_EDITIONS,
	)
	require.Equal(t, wantFeature, response.GetSupportedFeatures())
	wantEdition := int32(descriptorpb.Edition_EDITION_2024)
	require.Equal(t, wantEdition, response.GetMinimumEdition())
	require.Equal(t, wantEdition, response.GetMaximumEdition())
}

func TestEdition2024PresenceBecomesOption(t *testing.T) {
	request := scalarRequest()
	request.ProtoFile[0].Syntax = proto.String("editions")
	request.ProtoFile[0].Edition = descriptorpb.Edition_EDITION_2024.Enum()
	request.ProtoFile[0].MessageType[0].Field = request.ProtoFile[0].MessageType[0].Field[:1]

	requireGeneratedSnapshot(t, "edition-2024-presence", runRequest(t, request))
}

func TestDuplicateInputPathsAreIdempotent(t *testing.T) {
	request := scalarRequest()
	request.FileToGenerate = append(request.FileToGenerate, request.FileToGenerate[0])
	requireGeneratedSnapshot(t, "duplicate-input-paths", runRequest(t, request))
}

func TestFilesInSamePackageAggregateIntoMessages(t *testing.T) {
	requireGeneratedSnapshot(t, "same-package", runRequest(t, samePackageRequest()))
}

func TestNestedMessagesAndReferencesGenerateSnapshot(t *testing.T) {
	requireGeneratedSnapshot(t, "nested-messages", runRequest(t, nestedMessagesRequest()))
}

func TestRequiredNestedMessageReferenceGeneratesSnapshot(t *testing.T) {
	requireGeneratedSnapshot(t, "nested-required", runRequest(t, requiredNestedMessageRequest()))
}

func TestNestedRecordNameCollisionIsRejected(t *testing.T) {
	order := messageWithID("Order")
	order.NestedType = []*descriptorpb.DescriptorProto{messageWithID("LineItem")}
	request := requestForFiles(syntheticFile(
		"collision.proto",
		"example.events.v1",
		order,
		messageWithID("OrderLineItem"),
	))

	response := runRequest(t, request)
	require.Equal(
		t,
		"example.events.v1.OrderLineItem: normalized WIT name collision: example.events.v1.Order.LineItem",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "name collisions must not produce files")

	reversed := proto.Clone(request).(*pluginpb.CodeGeneratorRequest)
	reverse(reversed.ProtoFile[0].MessageType)
	require.True(
		t,
		proto.Equal(response, runRequest(t, reversed)),
		"record collision response depends on declaration order",
	)
}

func TestFieldNameCollisionIsIndependentOfDeclarationOrder(t *testing.T) {
	message := &descriptorpb.DescriptorProto{
		Name: proto.String("Sample"),
		Field: []*descriptorpb.FieldDescriptorProto{
			field(
				"URLValue",
				2,
				descriptorpb.FieldDescriptorProto_TYPE_STRING,
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
			field(
				"url_value",
				1,
				descriptorpb.FieldDescriptorProto_TYPE_STRING,
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
		},
	}
	request := requestForFiles(syntheticFile("collision.proto", "example.events.v1", message))
	response := runRequest(t, request)
	require.Equal(
		t,
		"example.events.v1.Sample.URLValue: normalized WIT name collision: example.events.v1.Sample.url_value",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "field name collisions must not produce files")

	reversed := proto.Clone(request).(*pluginpb.CodeGeneratorRequest)
	reverse(reversed.ProtoFile[0].MessageType[0].Field)
	require.True(
		t,
		proto.Equal(response, runRequest(t, reversed)),
		"field collision response depends on declaration order",
	)
}

func TestSelfRecursiveMessageIsRejected(t *testing.T) {
	node := &descriptorpb.DescriptorProto{
		Name: proto.String("Node"),
		Field: []*descriptorpb.FieldDescriptorProto{
			messageField(
				"next",
				1,
				".example.events.v1.Node",
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
		},
	}
	response := runRequest(t, requestForFiles(syntheticFile(
		"recursive.proto",
		"example.events.v1",
		node,
	)))

	require.Equal(
		t,
		"example.events.v1.Node: recursive message value types are not supported: example.events.v1.Node -> example.events.v1.Node",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "recursive schemas must not produce files")
}

func TestMutuallyRecursiveMessagesAreRejectedDeterministically(t *testing.T) {
	first := &descriptorpb.DescriptorProto{
		Name: proto.String("First"),
		Field: []*descriptorpb.FieldDescriptorProto{
			messageField(
				"second",
				1,
				".example.events.v1.Second",
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
		},
	}
	second := &descriptorpb.DescriptorProto{
		Name: proto.String("Second"),
		Field: []*descriptorpb.FieldDescriptorProto{
			messageField(
				"first",
				1,
				".example.events.v1.First",
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
		},
	}
	response := runRequest(t, requestForFiles(syntheticFile(
		"recursive.proto",
		"example.events.v1",
		second,
		first,
	)))

	require.Equal(
		t,
		"example.events.v1.First: recursive message value types are not supported: example.events.v1.First -> example.events.v1.Second -> example.events.v1.First",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "recursive schemas must not produce files")
}

func TestReferencedSamePackageImportIsIncluded(t *testing.T) {
	shared := messageWithID("Shared")
	shared.Field = append(shared.Field, messageField(
		"metadata",
		2,
		".example.events.v1.Metadata",
		descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
	))
	dependency := syntheticFile(
		"dependency.proto",
		"example.events.v1",
		shared,
		messageWithID("Metadata"),
	)
	requested := syntheticFile(
		"requested.proto",
		"example.events.v1",
		&descriptorpb.DescriptorProto{
			Name: proto.String("Visible"),
			Field: []*descriptorpb.FieldDescriptorProto{
				messageField(
					"shared",
					1,
					".example.events.v1.Shared",
					descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
				),
			},
		},
	)
	requested.Dependency = []string{dependency.GetName()}
	request := &pluginpb.CodeGeneratorRequest{
		FileToGenerate: []string{requested.GetName()},
		ProtoFile:      []*descriptorpb.FileDescriptorProto{dependency, requested},
	}

	requireGeneratedSnapshot(t, "referenced-import-inclusion", runRequest(t, request))
}

func TestCrossPackageMessageReferenceIsRejected(t *testing.T) {
	dependency := syntheticFile("shared.proto", "shared.types.v1", messageWithID("Shared"))
	requested := syntheticFile(
		"requested.proto",
		"example.events.v1",
		&descriptorpb.DescriptorProto{
			Name: proto.String("Visible"),
			Field: []*descriptorpb.FieldDescriptorProto{
				messageField(
					"shared",
					1,
					".shared.types.v1.Shared",
					descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
				),
			},
		},
	)
	requested.Dependency = []string{dependency.GetName()}
	request := &pluginpb.CodeGeneratorRequest{
		FileToGenerate: []string{requested.GetName()},
		ProtoFile:      []*descriptorpb.FileDescriptorProto{dependency, requested},
	}

	response := runRequest(t, request)
	require.Equal(
		t,
		"example.events.v1.Visible.shared: message reference is not part of the generated WIT package: shared.types.v1.Shared",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "cross-package message references must not produce files")
}

func TestMapFieldRemainsRejected(t *testing.T) {
	mapEntry := &descriptorpb.DescriptorProto{
		Name:    proto.String("EntriesEntry"),
		Options: &descriptorpb.MessageOptions{MapEntry: proto.Bool(true)},
		Field: []*descriptorpb.FieldDescriptorProto{
			field(
				"key",
				1,
				descriptorpb.FieldDescriptorProto_TYPE_STRING,
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
			field(
				"value",
				2,
				descriptorpb.FieldDescriptorProto_TYPE_STRING,
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
		},
	}
	catalog := &descriptorpb.DescriptorProto{
		Name:       proto.String("Catalog"),
		NestedType: []*descriptorpb.DescriptorProto{mapEntry},
		Field: []*descriptorpb.FieldDescriptorProto{
			messageField(
				"entries",
				1,
				".example.events.v1.Catalog.EntriesEntry",
				descriptorpb.FieldDescriptorProto_LABEL_REPEATED,
			),
		},
	}
	response := runRequest(t, requestForFiles(syntheticFile(
		"map.proto",
		"example.events.v1",
		catalog,
	)))

	require.Equal(t, "example.events.v1.Catalog.entries: maps are not supported", response.GetError())
	require.Empty(t, response.GetFile(), "map schemas must not produce files")
}

func TestProto2GroupRemainsRejectedAsUnsupportedFieldKind(t *testing.T) {
	group := messageWithID("LegacyGroup")
	groupType := descriptorpb.FieldDescriptorProto_TYPE_GROUP
	container := &descriptorpb.DescriptorProto{
		Name:       proto.String("Container"),
		NestedType: []*descriptorpb.DescriptorProto{group},
		Field: []*descriptorpb.FieldDescriptorProto{
			{
				Name:     proto.String("legacygroup"),
				Number:   proto.Int32(1),
				Type:     &groupType,
				TypeName: proto.String(".example.events.v1.Container.LegacyGroup"),
				Label:    descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL.Enum(),
			},
		},
	}
	file := syntheticFile("group.proto", "example.events.v1", container)
	file.Syntax = proto.String("proto2")
	response := runRequest(t, requestForFiles(file))

	require.Equal(
		t,
		"example.events.v1.Container.legacygroup: field kind is not supported: group",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "group schemas must not produce files")
}

func TestNormalizedRecordCollisionAcrossFilesIsRejected(t *testing.T) {
	request := requestForFiles(
		syntheticFile("first.proto", "example.events.v1", messageWithID("FooBar")),
		syntheticFile("second.proto", "example.events.v1", messageWithID("Foo_Bar")),
	)

	response := runRequest(t, request)
	require.Equal(
		t,
		"example.events.v1.Foo_Bar: normalized WIT name collision: example.events.v1.FooBar",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "name collisions must not produce files")
}

func TestMultiplePackagesGenerateInDeterministicPathOrder(t *testing.T) {
	request := multiplePackagesRequest()
	response := runRequest(t, request)

	requireGeneratedSnapshot(t, "multiple-packages", response)
	require.Equal(t, []string{
		"alpha/messages-v1/messages.wit",
		"omega/messages-v1/messages.wit",
	}, generatedFileNames(response))
}

func TestImportedButNotRequestedFileIsExcluded(t *testing.T) {
	dependency := syntheticFile(
		"dependency.proto",
		"example.events.v1",
		messageWithID("HiddenDependency"),
	)
	requested := syntheticFile(
		"requested.proto",
		"example.events.v1",
		messageWithID("VisibleMessage"),
	)
	requested.Dependency = []string{dependency.GetName()}
	request := &pluginpb.CodeGeneratorRequest{
		FileToGenerate: []string{requested.GetName()},
		ProtoFile:      []*descriptorpb.FileDescriptorProto{dependency, requested},
	}

	requireGeneratedSnapshot(t, "imported-file-exclusion", runRequest(t, request))
}

func TestShuffledRequestProducesIdenticalResponse(t *testing.T) {
	request := samePackageRequest()
	baseline := runRequest(t, request)

	shuffled := proto.Clone(request).(*pluginpb.CodeGeneratorRequest)
	reverse(shuffled.FileToGenerate)
	reverse(shuffled.ProtoFile)
	for _, file := range shuffled.ProtoFile {
		reverse(file.MessageType)
	}

	response := runRequest(t, shuffled)
	require.True(t, proto.Equal(baseline, response), "shuffled request produced a different response")
	requireGeneratedSnapshot(t, "same-package", response)
}

func TestUnsupportedFileKeepsResponseAtomic(t *testing.T) {
	valid := syntheticFile("valid.proto", "alpha.events.v1", messageWithID("ValidMessage"))
	unsupported := syntheticFile("unsupported.proto", "zeta.events.v1", messageWithID("UnsupportedMessage"))
	unsupported.EnumType = []*descriptorpb.EnumDescriptorProto{
		{
			Name: proto.String("Status"),
			Value: []*descriptorpb.EnumValueDescriptorProto{
				{Name: proto.String("STATUS_UNSPECIFIED"), Number: proto.Int32(0)},
			},
		},
	}

	response := runRequest(t, requestForFiles(valid, unsupported))
	require.Equal(t, "zeta.events.v1.Status: enums are not supported", response.GetError())
	require.Empty(t, response.GetFile(), "a later schema error must discard earlier package output")
}

func TestProto3OptionalBecomesOption(t *testing.T) {
	request := scalarRequest()
	message := request.ProtoFile[0].MessageType[0]
	message.Field = message.Field[:1]
	message.OneofDecl = []*descriptorpb.OneofDescriptorProto{{Name: proto.String("_score")}}
	message.Field[0].OneofIndex = proto.Int32(0)
	message.Field[0].Proto3Optional = proto.Bool(true)

	requireGeneratedSnapshot(t, "proto3-optional", runRequest(t, request))
}

func TestOneofVariantNameCollisionWithRecordIsRejected(t *testing.T) {
	container := scalarOneofMessage("Envelope", "Choice")
	request := requestForFiles(syntheticFile(
		"collision.proto",
		"example.events.v1",
		container,
		messageWithID("EnvelopeChoice"),
	))
	response := runRequest(t, request)
	require.Equal(
		t,
		"example.events.v1.Envelope.Choice: normalized WIT name collision: example.events.v1.EnvelopeChoice",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "type name collisions must not produce files")

	reversed := proto.Clone(request).(*pluginpb.CodeGeneratorRequest)
	reverse(reversed.ProtoFile[0].MessageType)
	require.True(t, proto.Equal(response, runRequest(t, reversed)), "type collision depends on declaration order")
}

func TestOneofVariantNameCollisionIsRejectedDeterministically(t *testing.T) {
	message := &descriptorpb.DescriptorProto{
		Name: proto.String("Envelope"),
		OneofDecl: []*descriptorpb.OneofDescriptorProto{
			{Name: proto.String("URLValue")},
			{Name: proto.String("url_value")},
		},
		Field: []*descriptorpb.FieldDescriptorProto{
			oneofField("first", 1, descriptorpb.FieldDescriptorProto_TYPE_STRING, 0),
			oneofField("second", 2, descriptorpb.FieldDescriptorProto_TYPE_STRING, 1),
		},
	}
	response := runRequest(t, requestForFiles(syntheticFile("collision.proto", "example.events.v1", message)))
	require.Equal(
		t,
		"example.events.v1.Envelope.url_value: normalized WIT name collision: example.events.v1.Envelope.URLValue",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "variant name collisions must not produce files")
}

func TestOneofFieldNameCollisionIsRejected(t *testing.T) {
	message := scalarOneofMessage("Envelope", "Choice")
	message.Field[2].Name = proto.String("choice")
	response := runRequest(t, requestForFiles(syntheticFile("collision.proto", "example.events.v1", message)))
	require.Equal(
		t,
		"example.events.v1.Envelope.Choice: normalized WIT name collision: example.events.v1.Envelope.choice",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "record field collisions must not produce files")
}

func TestOneofCaseNameCollisionIsRejectedDeterministically(t *testing.T) {
	message := &descriptorpb.DescriptorProto{
		Name:      proto.String("Envelope"),
		OneofDecl: []*descriptorpb.OneofDescriptorProto{{Name: proto.String("choice")}},
		Field: []*descriptorpb.FieldDescriptorProto{
			oneofField("URLValue", 2, descriptorpb.FieldDescriptorProto_TYPE_STRING, 0),
			oneofField("url_value", 1, descriptorpb.FieldDescriptorProto_TYPE_STRING, 0),
		},
	}
	request := requestForFiles(syntheticFile("collision.proto", "example.events.v1", message))
	response := runRequest(t, request)
	require.Equal(
		t,
		"example.events.v1.Envelope.URLValue: normalized WIT name collision: example.events.v1.Envelope.url_value",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "variant case collisions must not produce files")

	reversed := proto.Clone(request).(*pluginpb.CodeGeneratorRequest)
	reverse(reversed.ProtoFile[0].MessageType[0].Field)
	require.True(t, proto.Equal(response, runRequest(t, reversed)), "case collision depends on declaration order")
}

func TestEmptyOneofIsRejected(t *testing.T) {
	message := &descriptorpb.DescriptorProto{
		Name:      proto.String("Envelope"),
		OneofDecl: []*descriptorpb.OneofDescriptorProto{{Name: proto.String("payload")}},
	}
	response := runRequest(t, requestForFiles(syntheticFile("empty.proto", "example.events.v1", message)))
	require.Contains(t, response.GetError(), "link protobuf descriptors")
	require.Contains(
		t,
		response.GetError(),
		"message oneof \"example.events.v1.Envelope.payload\" must contain at least one field declaration",
	)
	require.Empty(t, response.GetFile(), "empty oneofs must not produce invalid WIT variants")
}

func TestStandaloneEmptyMessageIsRejected(t *testing.T) {
	message := &descriptorpb.DescriptorProto{Name: proto.String("Heartbeat")}
	response := runRequest(t, requestForFiles(syntheticFile("empty.proto", "example.events.v1", message)))
	require.Equal(
		t,
		"example.events.v1.Heartbeat: messages without fields can only be represented as oneof members",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "empty messages must not produce invalid WIT records")
}

func TestEmptyMessageFieldIsRejected(t *testing.T) {
	container := &descriptorpb.DescriptorProto{
		Name:       proto.String("Envelope"),
		NestedType: []*descriptorpb.DescriptorProto{{Name: proto.String("Marker")}},
		Field: []*descriptorpb.FieldDescriptorProto{
			messageField(
				"marker",
				1,
				".example.events.v1.Envelope.Marker",
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
		},
	}
	response := runRequest(t, requestForFiles(syntheticFile("empty.proto", "example.events.v1", container)))
	require.Equal(
		t,
		"example.events.v1.Envelope.marker: messages without fields can only be represented as oneof members: example.events.v1.Envelope.Marker",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "empty message fields must not produce invalid WIT records")
}

func TestRecursiveMessageThroughOneofIsRejected(t *testing.T) {
	node := &descriptorpb.DescriptorProto{
		Name:      proto.String("Node"),
		OneofDecl: []*descriptorpb.OneofDescriptorProto{{Name: proto.String("link")}},
		Field: []*descriptorpb.FieldDescriptorProto{
			oneofMessageField("next", 1, ".example.events.v1.Node", 0),
		},
	}
	response := runRequest(t, requestForFiles(syntheticFile("recursive.proto", "example.events.v1", node)))
	require.Equal(
		t,
		"example.events.v1.Node: recursive message value types are not supported: example.events.v1.Node -> example.events.v1.Node",
		response.GetError(),
	)
	require.Empty(t, response.GetFile(), "recursive oneof schemas must not produce files")
}

func TestSchedulerEventProtosGenerateSnapshots(t *testing.T) {
	request := schedulerEventRequest(t, repositoryRoot(t))

	requireGeneratedSnapshot(t, "scheduler-events", runRequest(t, request))
}

func TestNormalizeIdentifier(t *testing.T) {
	tests := map[string]witIdentifier{
		"ErrorContext": "%error-context",
		"event_id":     "event-id",
		"URLValue":     "url-value",
		"type":         "%type",
		"v1Event":      "v1-event",
	}
	for input, want := range tests {
		got, err := normalizeIdentifier(input)
		require.NoError(t, err, "normalize %q", input)
		require.Equal(t, want, got, "normalize %q", input)
	}
}

func TestPackageIDMappingIsInjective(t *testing.T) {
	underscore, err := packageID("a.b_c")
	require.NoError(t, err, "map underscore package")
	segmented, err := packageID("a.b.c")
	require.NoError(t, err, "map segmented package")
	require.NotEqual(t, underscore, segmented, "package mappings must be injective")
	require.Equal(t, "a:bzx5fc", underscore.String())
	require.Equal(t, "a:b-c", segmented.String())
}

func requireGeneratedSnapshot(
	t *testing.T,
	caseName string,
	response *pluginpb.CodeGeneratorResponse,
) {
	t.Helper()
	require.Empty(t, response.GetError(), "code generator response error")

	snapshotRoot := filepath.Join("testdata", "snapshots", caseName)
	expected := make(map[string]string)
	err := filepath.WalkDir(snapshotRoot, func(path string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if entry.IsDir() {
			return nil
		}
		relativePath, err := filepath.Rel(snapshotRoot, path)
		if err != nil {
			return err
		}
		content, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		expected[filepath.ToSlash(relativePath)] = string(content)
		return nil
	})
	require.NoError(t, err, "read generated-file snapshot %q", caseName)
	require.NotEmpty(t, expected, "generated-file snapshot %q contains no files", caseName)

	expectedNames := make([]string, 0, len(expected))
	for name := range expected {
		expectedNames = append(expectedNames, name)
	}
	sort.Strings(expectedNames)

	actual := make(map[string]string, len(response.GetFile()))
	actualNames := make([]string, 0, len(response.GetFile()))
	for _, generatedFile := range response.GetFile() {
		name := generatedFile.GetName()
		actualNames = append(actualNames, name)
		actual[name] = generatedFile.GetContent()
	}
	sort.Strings(actualNames)
	assert.Equal(t, expectedNames, actualNames, "generated file names for snapshot %q", caseName)

	for _, name := range expectedNames {
		content, found := actual[name]
		if !assert.True(t, found, "generated file %q is missing", name) {
			continue
		}
		assert.Equal(t, expected[name], content, "generated file %q", name)
	}
}

func generatedFileNames(response *pluginpb.CodeGeneratorResponse) []string {
	names := make([]string, 0, len(response.GetFile()))
	for _, file := range response.GetFile() {
		names = append(names, file.GetName())
	}
	return names
}

func runRequest(t *testing.T, request *pluginpb.CodeGeneratorRequest) *pluginpb.CodeGeneratorResponse {
	t.Helper()
	requestBytes, err := proto.Marshal(request)
	require.NoError(t, err, "marshal request")
	var output bytes.Buffer
	require.NoError(t, Run(bytes.NewReader(requestBytes), &output), "run plugin")
	response := new(pluginpb.CodeGeneratorResponse)
	require.NoError(t, proto.Unmarshal(output.Bytes(), response), "unmarshal response")
	return response
}

func schedulerEventRequest(t *testing.T, repoRoot string) *pluginpb.CodeGeneratorRequest {
	t.Helper()
	bufPath, err := exec.LookPath("buf")
	require.NoError(
		t,
		err,
		"buf is required to compile repository protobuf fixtures; run tests through mise: mise exec -- go -C goworkspace test ./...",
	)

	descriptorPaths := []string{
		"trogonai/scheduler/schedules/v1/schedule_paused.proto",
		"trogonai/scheduler/schedules/v1/schedule_resumed.proto",
		"trogonai/scheduler/schedules/v1/schedule_removed.proto",
		"trogonai/scheduler/schedules/v1/schedule_completed.proto",
		"trogonai/scheduler/schedules/v1/schedule_status.proto",
	}
	arguments := []string{"build", "--as-file-descriptor-set", "--output", "-"}
	for _, descriptorPath := range descriptorPaths {
		arguments = append(arguments, "--path", "proto/"+descriptorPath)
	}

	var stdout bytes.Buffer
	var stderr bytes.Buffer
	command := exec.CommandContext(t.Context(), bufPath, arguments...)
	command.Dir = repoRoot
	command.Stdout = &stdout
	command.Stderr = &stderr
	require.NoErrorf(
		t,
		command.Run(),
		"build scheduler protobuf descriptor set with buf:\n%s",
		stderr.String(),
	)

	descriptorSet := new(descriptorpb.FileDescriptorSet)
	require.NoError(t, proto.Unmarshal(stdout.Bytes(), descriptorSet), "decode scheduler protobuf descriptor set")
	require.NotEmpty(t, descriptorSet.GetFile(), "scheduler protobuf descriptor set contains no files")

	return &pluginpb.CodeGeneratorRequest{
		FileToGenerate: descriptorPaths,
		ProtoFile:      descriptorSet.GetFile(),
	}
}

func repositoryRoot(t *testing.T) string {
	t.Helper()
	directory, err := os.Getwd()
	require.NoError(t, err, "get test working directory")
	start := directory

	for {
		bufConfig, bufErr := os.Stat(filepath.Join(directory, "buf.yaml"))
		goModule, goErr := os.Stat(filepath.Join(directory, "goworkspace", "go.mod"))
		if bufErr == nil && !bufConfig.IsDir() && goErr == nil && !goModule.IsDir() {
			return directory
		}

		parent := filepath.Dir(directory)
		if parent == directory {
			t.Fatalf(
				"locate repository root from %q: expected a parent containing buf.yaml and goworkspace/go.mod",
				start,
			)
		}
		directory = parent
	}
}

func scalarRequest() *pluginpb.CodeGeneratorRequest {
	optional := descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL
	repeated := descriptorpb.FieldDescriptorProto_LABEL_REPEATED
	return &pluginpb.CodeGeneratorRequest{
		FileToGenerate: []string{"event.proto"},
		ProtoFile: []*descriptorpb.FileDescriptorProto{
			{
				Name:    proto.String("event.proto"),
				Package: proto.String("example.events.v1"),
				Syntax:  proto.String("proto3"),
				MessageType: []*descriptorpb.DescriptorProto{
					{
						Name: proto.String("EventSample"),
						Field: []*descriptorpb.FieldDescriptorProto{
							field("score", 6, descriptorpb.FieldDescriptorProto_TYPE_DOUBLE, optional),
							field("event_id", 1, descriptorpb.FieldDescriptorProto_TYPE_STRING, optional),
							field("active", 5, descriptorpb.FieldDescriptorProto_TYPE_BOOL, optional),
							field("payload", 3, descriptorpb.FieldDescriptorProto_TYPE_BYTES, optional),
							field("tags", 4, descriptorpb.FieldDescriptorProto_TYPE_STRING, repeated),
							field("sequence", 2, descriptorpb.FieldDescriptorProto_TYPE_INT64, optional),
						},
					},
				},
			},
		},
	}
}

func samePackageRequest() *pluginpb.CodeGeneratorRequest {
	return requestForFiles(
		syntheticFile(
			"second.proto",
			"example.events.v1",
			messageWithID("ThirdMessage"),
			messageWithID("SecondMessage"),
		),
		syntheticFile("first.proto", "example.events.v1", messageWithID("FirstMessage")),
	)
}

func scalarOneofRequest() *pluginpb.CodeGeneratorRequest {
	return requestForFiles(syntheticFile(
		"choice.proto",
		"example.events.v1",
		scalarOneofMessage("Envelope", "payload"),
	))
}

func scalarOneofMessage(name string, oneofName string) *descriptorpb.DescriptorProto {
	return &descriptorpb.DescriptorProto{
		Name:      proto.String(name),
		OneofDecl: []*descriptorpb.OneofDescriptorProto{{Name: proto.String(oneofName)}},
		Field: []*descriptorpb.FieldDescriptorProto{
			oneofField("text", 2, descriptorpb.FieldDescriptorProto_TYPE_STRING, 0),
			oneofField("sequence", 3, descriptorpb.FieldDescriptorProto_TYPE_INT64, 0),
			field("request_id", 1, descriptorpb.FieldDescriptorProto_TYPE_STRING, descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL),
		},
	}
}

func messageOneofRequest() *pluginpb.CodeGeneratorRequest {
	envelope := &descriptorpb.DescriptorProto{
		Name: proto.String("Envelope"),
		NestedType: []*descriptorpb.DescriptorProto{
			{Name: proto.String("Created")},
			{Name: proto.String("Deleted")},
		},
		OneofDecl: []*descriptorpb.OneofDescriptorProto{{Name: proto.String("body")}},
		Field: []*descriptorpb.FieldDescriptorProto{
			oneofMessageField("created", 1, ".example.events.v1.Envelope.Created", 0),
			oneofMessageField("deleted", 2, ".example.events.v1.Envelope.Deleted", 0),
		},
	}
	return requestForFiles(syntheticFile("message.proto", "example.events.v1", envelope))
}

func multipleOneofsRequest() *pluginpb.CodeGeneratorRequest {
	message := &descriptorpb.DescriptorProto{
		Name: proto.String("Transfer"),
		NestedType: []*descriptorpb.DescriptorProto{
			messageWithID("Alias"),
		},
		OneofDecl: []*descriptorpb.OneofDescriptorProto{
			{Name: proto.String("source")},
			{Name: proto.String("destination")},
		},
		Field: []*descriptorpb.FieldDescriptorProto{
			field("request_id", 1, descriptorpb.FieldDescriptorProto_TYPE_STRING, descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL),
			oneofField("source_account", 2, descriptorpb.FieldDescriptorProto_TYPE_STRING, 0),
			oneofMessageField("source_alias", 4, ".example.events.v1.Transfer.Alias", 0),
			oneofField("destination_account", 3, descriptorpb.FieldDescriptorProto_TYPE_STRING, 1),
			oneofMessageField("destination_alias", 5, ".example.events.v1.Transfer.Alias", 1),
		},
	}
	return requestForFiles(syntheticFile("multiple.proto", "example.events.v1", message))
}

func importedOneofRequest() *pluginpb.CodeGeneratorRequest {
	shared := &descriptorpb.DescriptorProto{
		Name: proto.String("Shared"),
		Field: []*descriptorpb.FieldDescriptorProto{
			messageField("metadata", 1, ".example.events.v1.Metadata", descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL),
		},
	}
	dependency := syntheticFile(
		"dependency.proto",
		"example.events.v1",
		shared,
		messageWithID("Metadata"),
	)
	envelope := &descriptorpb.DescriptorProto{
		Name:      proto.String("Envelope"),
		OneofDecl: []*descriptorpb.OneofDescriptorProto{{Name: proto.String("payload")}},
		Field: []*descriptorpb.FieldDescriptorProto{
			oneofMessageField("shared", 1, ".example.events.v1.Shared", 0),
			oneofField("raw", 2, descriptorpb.FieldDescriptorProto_TYPE_BYTES, 0),
		},
	}
	requested := syntheticFile("requested.proto", "example.events.v1", envelope)
	requested.Dependency = []string{dependency.GetName()}
	return &pluginpb.CodeGeneratorRequest{
		FileToGenerate: []string{requested.GetName()},
		ProtoFile:      []*descriptorpb.FileDescriptorProto{dependency, requested},
	}
}

func nestedMessagesRequest() *pluginpb.CodeGeneratorRequest {
	metadata := &descriptorpb.DescriptorProto{
		Name: proto.String("Metadata"),
		Field: []*descriptorpb.FieldDescriptorProto{
			field(
				"note",
				1,
				descriptorpb.FieldDescriptorProto_TYPE_STRING,
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
		},
	}
	lineItem := &descriptorpb.DescriptorProto{
		Name:       proto.String("LineItem"),
		NestedType: []*descriptorpb.DescriptorProto{metadata},
		Field: []*descriptorpb.FieldDescriptorProto{
			field(
				"sku",
				1,
				descriptorpb.FieldDescriptorProto_TYPE_STRING,
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
			messageField(
				"metadata",
				2,
				".example.events.v1.Order.LineItem.Metadata",
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
		},
	}
	order := &descriptorpb.DescriptorProto{
		Name:       proto.String("Order"),
		NestedType: []*descriptorpb.DescriptorProto{lineItem},
		Field: []*descriptorpb.FieldDescriptorProto{
			messageField(
				"customer",
				1,
				".example.events.v1.Customer",
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
			messageField(
				"primary_item",
				2,
				".example.events.v1.Order.LineItem",
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
			messageField(
				"items",
				3,
				".example.events.v1.Order.LineItem",
				descriptorpb.FieldDescriptorProto_LABEL_REPEATED,
			),
		},
	}
	return requestForFiles(syntheticFile(
		"nested.proto",
		"example.events.v1",
		order,
		messageWithID("Customer"),
	))
}

func requiredNestedMessageRequest() *pluginpb.CodeGeneratorRequest {
	container := &descriptorpb.DescriptorProto{
		Name: proto.String("Container"),
		NestedType: []*descriptorpb.DescriptorProto{
			messageWithID("Item"),
		},
		Field: []*descriptorpb.FieldDescriptorProto{
			messageField(
				"item",
				1,
				".example.events.v1.Container.Item",
				descriptorpb.FieldDescriptorProto_LABEL_REQUIRED,
			),
		},
	}
	file := syntheticFile("required.proto", "example.events.v1", container)
	file.Syntax = proto.String("proto2")
	return requestForFiles(file)
}

func multiplePackagesRequest() *pluginpb.CodeGeneratorRequest {
	return requestForFiles(
		syntheticFile("omega.proto", "omega.messages.v1", messageWithID("OmegaMessage")),
		syntheticFile("alpha.proto", "alpha.messages.v1", messageWithID("AlphaMessage")),
	)
}

func requestForFiles(files ...*descriptorpb.FileDescriptorProto) *pluginpb.CodeGeneratorRequest {
	paths := make([]string, 0, len(files))
	for _, file := range files {
		paths = append(paths, file.GetName())
	}
	return &pluginpb.CodeGeneratorRequest{FileToGenerate: paths, ProtoFile: files}
}

func syntheticFile(
	path string,
	protobufPackage string,
	messages ...*descriptorpb.DescriptorProto,
) *descriptorpb.FileDescriptorProto {
	return &descriptorpb.FileDescriptorProto{
		Name:        proto.String(path),
		Package:     proto.String(protobufPackage),
		Syntax:      proto.String("proto3"),
		MessageType: messages,
	}
}

func messageWithID(name string) *descriptorpb.DescriptorProto {
	return &descriptorpb.DescriptorProto{
		Name: proto.String(name),
		Field: []*descriptorpb.FieldDescriptorProto{
			field(
				"id",
				1,
				descriptorpb.FieldDescriptorProto_TYPE_STRING,
				descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL,
			),
		},
	}
}

func reverse[T any](values []T) {
	for left, right := 0, len(values)-1; left < right; left, right = left+1, right-1 {
		values[left], values[right] = values[right], values[left]
	}
}

func field(
	name string,
	number int32,
	typeID descriptorpb.FieldDescriptorProto_Type,
	label descriptorpb.FieldDescriptorProto_Label,
) *descriptorpb.FieldDescriptorProto {
	return &descriptorpb.FieldDescriptorProto{
		Name:   proto.String(name),
		Number: proto.Int32(number),
		Type:   &typeID,
		Label:  &label,
	}
}

func messageField(
	name string,
	number int32,
	typeName string,
	label descriptorpb.FieldDescriptorProto_Label,
) *descriptorpb.FieldDescriptorProto {
	message := descriptorpb.FieldDescriptorProto_TYPE_MESSAGE
	return &descriptorpb.FieldDescriptorProto{
		Name:     proto.String(name),
		Number:   proto.Int32(number),
		Type:     &message,
		TypeName: proto.String(typeName),
		Label:    &label,
	}
}

func oneofField(
	name string,
	number int32,
	typeID descriptorpb.FieldDescriptorProto_Type,
	oneofIndex int32,
) *descriptorpb.FieldDescriptorProto {
	result := field(name, number, typeID, descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL)
	result.OneofIndex = proto.Int32(oneofIndex)
	return result
}

func oneofMessageField(name string, number int32, typeName string, oneofIndex int32) *descriptorpb.FieldDescriptorProto {
	result := messageField(name, number, typeName, descriptorpb.FieldDescriptorProto_LABEL_OPTIONAL)
	result.OneofIndex = proto.Int32(oneofIndex)
	return result
}
