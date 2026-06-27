package before_resolution

import rego.v1

# Encodes the enforceable subset of the trogonstack-otel `otel-name-metric`
# conventions (metric names and attribute ids). Span-name enforcement is not
# implemented. Findings are evaluated against the raw (unresolved) registry:
# `input.groups` are the authored groups.

# Registered OpenTelemetry root namespaces. Custom telemetry must not claim them.
otel_reserved_roots := {
	"http", "rpc", "messaging", "dns", "db", "system", "process", "container",
	"k8s", "jvm", "dotnet", "nodejs", "go", "v8js", "aws", "azure", "gcp",
	"faas", "gen_ai", "cicd", "kestrel", "signalr", "aspnetcore",
}

deny contains finding("metric_name_not_lowercase", message, "violation") if {
	group := input.groups[_]
	group.type == "metric"
	name := group.metric_name
	lower(name) != name
	message := sprintf("metric name '%s' must be lowercase", [name])
}

deny contains finding("metric_name_total_suffix", message, "violation") if {
	group := input.groups[_]
	group.type == "metric"
	endswith(group.metric_name, "_total")
	message := sprintf("metric name '%s' must not end with '_total'", [group.metric_name])
}

deny contains finding("metric_name_reserved_otel_namespace", message, "violation") if {
	group := input.groups[_]
	group.type == "metric"
	startswith(group.metric_name, "otel.")
	message := sprintf("metric name '%s' must not use the reserved 'otel.' namespace", [group.metric_name])
}

deny contains finding("metric_name_consecutive_delimiters", message, "violation") if {
	group := input.groups[_]
	group.type == "metric"
	regex.match(`\.\.|__|\._|_\.`, group.metric_name)
	message := sprintf("metric name '%s' must not contain consecutive delimiters", [group.metric_name])
}

deny contains finding("metric_name_format", message, "violation") if {
	group := input.groups[_]
	group.type == "metric"
	not regex.match(`^[a-z][a-z0-9_.]*[a-z0-9]$`, group.metric_name)
	message := sprintf("metric name '%s' must match ^[a-z][a-z0-9_.]*[a-z0-9]$", [group.metric_name])
}

deny contains finding("metric_name_reserved_root", message, "violation") if {
	group := input.groups[_]
	group.type == "metric"
	root := split(group.metric_name, ".")[0]
	otel_reserved_roots[root]
	message := sprintf("metric name '%s' claims reserved OTel root namespace '%s'; custom metrics must use a non-reserved root", [group.metric_name, root])
}

deny contains finding("metric_unit_missing", message, "violation") if {
	group := input.groups[_]
	group.type == "metric"
	not group.unit
	message := sprintf("metric '%s' must declare a unit (units belong in metadata, not the name)", [group.metric_name])
}

deny contains finding("attribute_id_format", message, "violation") if {
	group := input.groups[_]
	attr := group.attributes[_]
	attr.id
	not regex.match(`^[a-z][a-z0-9_.]*$`, attr.id)
	message := sprintf("attribute id '%s' must be dot-delimited snake_case", [attr.id])
}

deny contains finding("attribute_id_reserved_otel_namespace", message, "violation") if {
	group := input.groups[_]
	attr := group.attributes[_]
	startswith(attr.id, "otel.")
	message := sprintf("attribute id '%s' must not use the reserved 'otel.' namespace", [attr.id])
}

finding(id, message, level) := {
	"id": id,
	"message": message,
	"level": level,
}
