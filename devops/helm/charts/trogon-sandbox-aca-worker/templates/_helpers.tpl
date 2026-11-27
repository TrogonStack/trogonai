{{- define "trogon-sandbox-aca-worker.fullname" -}}
{{- printf "%s-sandbox-aca-worker" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "trogon-sandbox-aca-worker.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: worker-aca
app.kubernetes.io/name: sandbox-aca-worker
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}

{{- define "trogon-sandbox-aca-worker.natsUrl" -}}
{{- if .Values.global.natsUrl -}}
{{- .Values.global.natsUrl -}}
{{- else -}}
nats://{{ .Release.Name }}-nats:4222
{{- end -}}
{{- end }}

{{- define "trogon-sandbox-aca-worker.registryUrl" -}}
{{- if .Values.global.registryUrl -}}
{{- .Values.global.registryUrl -}}
{{- else -}}
{{ .Release.Name }}-zot:5000
{{- end -}}
{{- end }}
