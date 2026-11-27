{{- define "nats.fullname" -}}
{{- printf "%s-nats" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "nats.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: nats
app.kubernetes.io/name: nats
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}
