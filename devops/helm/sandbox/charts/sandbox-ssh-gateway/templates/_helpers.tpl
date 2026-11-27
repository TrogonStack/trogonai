{{- define "sandbox-ssh-gateway.fullname" -}}
{{- printf "%s-sandbox-ssh-gateway" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "sandbox-ssh-gateway.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: ssh-gateway
app.kubernetes.io/name: sandbox-ssh-gateway
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}

{{- define "sandbox-ssh-gateway.natsUrl" -}}
{{- if .Values.global.natsUrl -}}
{{- .Values.global.natsUrl -}}
{{- else -}}
nats://{{ .Release.Name }}-nats:4222
{{- end -}}
{{- end }}

{{- define "sandbox-ssh-gateway.registryUrl" -}}
{{- if .Values.global.registryUrl -}}
{{- .Values.global.registryUrl -}}
{{- else -}}
{{ .Release.Name }}-zot:5000
{{- end -}}
{{- end }}
