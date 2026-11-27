{{- define "trogon-sandbox-ws-gateway.fullname" -}}
{{- printf "%s-sandbox-ws-gateway" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "trogon-sandbox-ws-gateway.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: ws-gateway
app.kubernetes.io/name: sandbox-ws-gateway
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}

{{- define "trogon-sandbox-ws-gateway.natsUrl" -}}
{{- if .Values.global.natsUrl -}}
{{- .Values.global.natsUrl -}}
{{- else -}}
nats://{{ .Release.Name }}-nats:4222
{{- end -}}
{{- end }}

{{- define "trogon-sandbox-ws-gateway.registryUrl" -}}
{{- if .Values.global.registryUrl -}}
{{- .Values.global.registryUrl -}}
{{- else -}}
{{ .Release.Name }}-zot:5000
{{- end -}}
{{- end }}
