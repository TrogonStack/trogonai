{{- define "sandbox-snapshot-builder.fullname" -}}
{{- printf "%s-sandbox-snapshot-builder" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "sandbox-snapshot-builder.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: snapshot-builder
app.kubernetes.io/name: sandbox-snapshot-builder
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}

{{- define "sandbox-snapshot-builder.natsUrl" -}}
{{- if .Values.global.natsUrl -}}
{{- .Values.global.natsUrl -}}
{{- else -}}
nats://{{ .Release.Name }}-nats:4222
{{- end -}}
{{- end }}

{{- define "sandbox-snapshot-builder.registryUrl" -}}
{{- if .Values.global.registryUrl -}}
{{- .Values.global.registryUrl -}}
{{- else -}}
{{ .Release.Name }}-zot:5000
{{- end -}}
{{- end }}
