package protowit

import (
	"fmt"
	"io"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/descriptorpb"
	"google.golang.org/protobuf/types/pluginpb"
)

func Run(input io.Reader, output io.Writer) error {
	requestBytes, err := io.ReadAll(input)
	if err != nil {
		return fmt.Errorf("read code generator request: %w", err)
	}

	request := new(pluginpb.CodeGeneratorRequest)
	if err := proto.Unmarshal(requestBytes, request); err != nil {
		return fmt.Errorf("decode code generator request: %w", err)
	}

	responseBytes, err := proto.MarshalOptions{Deterministic: true}.Marshal(generate(request))
	if err != nil {
		return fmt.Errorf("encode code generator response: %w", err)
	}
	if _, err := output.Write(responseBytes); err != nil {
		return fmt.Errorf("write code generator response: %w", err)
	}

	return nil
}

func generate(request *pluginpb.CodeGeneratorRequest) *pluginpb.CodeGeneratorResponse {
	response := edition2024Response()
	files, err := generateFiles(request)
	if err != nil {
		response.Error = proto.String(err.Error())
		return response
	}
	response.File = files
	return response
}

func edition2024Response() *pluginpb.CodeGeneratorResponse {
	features := uint64(
		pluginpb.CodeGeneratorResponse_FEATURE_PROTO3_OPTIONAL |
			pluginpb.CodeGeneratorResponse_FEATURE_SUPPORTS_EDITIONS,
	)
	edition := int32(descriptorpb.Edition_EDITION_2024)
	return &pluginpb.CodeGeneratorResponse{
		SupportedFeatures: &features,
		MinimumEdition:    &edition,
		MaximumEdition:    &edition,
	}
}
