{{- define "sandbox-k8s-worker.fullname" -}}
{{- printf "%s-sandbox-k8s-worker" .Release.Name | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "sandbox-k8s-worker.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ .Values.global.stackLabel }}: {{ .Values.global.stackLabelValue | quote }}
{{ .Values.global.componentLabel }}: worker-k8s
app.kubernetes.io/name: sandbox-k8s-worker
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/part-of: sandbox-stack
{{- end }}

{{- define "sandbox-k8s-worker.natsUrl" -}}
{{- if .Values.global.natsUrl -}}
{{- .Values.global.natsUrl -}}
{{- else -}}
nats://{{ .Release.Name }}-nats:4222
{{- end -}}
{{- end }}

{{- define "sandbox-k8s-worker.registryUrl" -}}
{{- if .Values.global.registryUrl -}}
{{- .Values.global.registryUrl -}}
{{- else -}}
{{ .Release.Name }}-zot:5000
{{- end -}}
{{- end }}

{{- /*
The StatefulSet's name. The controller-revision-hash label the StatefulSet
controller puts on each pod is this name plus an 11 character suffix, and a
label value is at most 63 characters, so the name stops at 52. The pod name,
which becomes the worker identity label, then fits as well.
*/}}
{{- define "sandbox-k8s-worker.statefulSetName" -}}
{{- include "sandbox-k8s-worker.fullname" . | trunc 52 | trimSuffix "-" }}
{{- end }}

{{- define "sandbox-k8s-worker.validateWorkload" -}}
{{- if not (has .Values.workloadKind (list "Deployment" "StatefulSet")) -}}
{{- fail (printf "sandbox-k8s-worker.workloadKind must be Deployment or StatefulSet, got %q" .Values.workloadKind) -}}
{{- end -}}
{{- if and (eq .Values.workloadKind "Deployment") (gt (int .Values.replicaCount) 1) -}}
{{- fail "sandbox-k8s-worker.replicaCount above 1 needs workloadKind: StatefulSet, so each replica gets its own worker identity" -}}
{{- end -}}
{{- end }}

{{- define "sandbox-k8s-worker.podTemplate" -}}
metadata:
  labels:
    {{- include "sandbox-k8s-worker.labels" . | nindent 4 }}
    sandbox.trogonai.io/egress-enforcement: {{ .Values.egressEnforcement | quote }}
spec:
  serviceAccountName: {{ include "sandbox-k8s-worker.fullname" . }}
  containers:
    - name: worker
      image: "{{ .Values.global.imageRegistry }}/trogon-sandbox-k8s-worker:{{ .Values.global.imageTag }}"
      imagePullPolicy: {{ .Values.global.imagePullPolicy }}
      envFrom:
        - configMapRef:
            name: {{ include "sandbox-k8s-worker.fullname" . }}
      ports:
        - name: metrics
          containerPort: {{ .Values.metricsPort }}
      {{- if or (eq .Values.workloadKind "StatefulSet") .Values.global.dispatchAuth }}
      env:
      {{- if eq .Values.workloadKind "StatefulSet" }}
        # The pod name is stable across restarts of the same ordinal, so a
        # restarted replica recovers the leases it launched before.
        - name: SANDBOX_K8S_WORKER_IDENTITY
          valueFrom:
            fieldRef:
              fieldPath: metadata.name
        - name: SANDBOX_K8S_RECOVER_UNLABELED_LEASES
          value: {{ .Values.recoverUnlabeledLeases | toString | quote }}
      {{- end }}
      {{- with .Values.global.dispatchAuth }}
        - name: SANDBOX_DISPATCH_PUBLIC_KEYS
          valueFrom:
            secretKeyRef:
              name: {{ .secretName }}
              key: {{ .publicKeysKey }}
      {{- end }}
      {{- end }}
      resources:
        {{- toYaml .Values.resources | nindent 8 }}
      livenessProbe:
        exec:
          command: ["/usr/local/bin/trogon-sandbox-k8s-worker", "--help"]
        initialDelaySeconds: 20
        periodSeconds: 30
      readinessProbe:
        exec:
          command: ["/usr/local/bin/trogon-sandbox-k8s-worker", "--help"]
        initialDelaySeconds: 10
        periodSeconds: 15
{{- end }}
