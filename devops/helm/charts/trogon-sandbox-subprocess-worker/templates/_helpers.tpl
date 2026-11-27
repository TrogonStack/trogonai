{{- define "trogon-sandbox-subprocess-worker.fullname" -}}
{{- printf "%s-sandbox-subprocess-worker" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "trogon-sandbox-subprocess-worker.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: worker-subprocess
app.kubernetes.io/name: sandbox-subprocess-worker
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}

{{- define "trogon-sandbox-subprocess-worker.natsUrl" -}}
{{- if .Values.global.natsUrl -}}
{{- .Values.global.natsUrl -}}
{{- else -}}
nats://{{ .Release.Name }}-nats:4222
{{- end -}}
{{- end }}

{{- define "trogon-sandbox-subprocess-worker.registryUrl" -}}
{{- if .Values.global.registryUrl -}}
{{- .Values.global.registryUrl -}}
{{- else -}}
{{ .Release.Name }}-zot:5000
{{- end -}}
{{- end }}
