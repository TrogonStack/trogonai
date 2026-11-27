{{- define "trogon-sandbox-docker-worker.fullname" -}}
{{- printf "%s-sandbox-docker-worker" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "trogon-sandbox-docker-worker.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: worker-docker
app.kubernetes.io/name: sandbox-docker-worker
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}

{{- define "trogon-sandbox-docker-worker.natsUrl" -}}
{{- if .Values.global.natsUrl -}}
{{- .Values.global.natsUrl -}}
{{- else -}}
nats://{{ .Release.Name }}-nats:4222
{{- end -}}
{{- end }}

{{- define "trogon-sandbox-docker-worker.registryUrl" -}}
{{- if .Values.global.registryUrl -}}
{{- .Values.global.registryUrl -}}
{{- else -}}
{{ .Release.Name }}-zot:5000
{{- end -}}
{{- end }}
