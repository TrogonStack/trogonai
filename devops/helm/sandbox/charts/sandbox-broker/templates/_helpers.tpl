{{- define "sandbox-broker.fullname" -}}
{{- printf "%s-sandbox-broker" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "sandbox-broker.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: broker
app.kubernetes.io/name: sandbox-broker
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}

{{- define "sandbox-broker.natsUrl" -}}
{{- if .Values.global.natsUrl -}}
{{- .Values.global.natsUrl -}}
{{- else -}}
nats://{{ .Release.Name }}-nats:4222
{{- end -}}
{{- end }}

{{- define "sandbox-broker.spicedbEndpoint" -}}
{{- if .Values.global.spicedb.endpoint -}}
{{- .Values.global.spicedb.endpoint -}}
{{- else -}}
{{ .Release.Name }}-spicedb:50051
{{- end -}}
{{- end }}

{{- define "sandbox-broker.registryUrl" -}}
{{- if .Values.global.registryUrl -}}
{{- .Values.global.registryUrl -}}
{{- else -}}
{{ .Release.Name }}-zot:5000
{{- end -}}
{{- end }}
