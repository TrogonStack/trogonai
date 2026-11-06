package policytest

import (
	"encoding/json"
	"fmt"
	"os/exec"
	"strings"
	"testing"

	"github.com/google/cel-go/cel"
	"github.com/google/cel-go/common/types"
)

type policyDocument struct {
	Resources map[string]any   `json:"resources"`
	Patches   []map[string]any `json:"patches"`
}

func generated(t *testing.T) policyDocument {
	t.Helper()
	code := `import json,sys
from pathlib import Path
sys.path.insert(0,"..")
from render import render
files=render(json.loads(Path("../deployment.example.json").read_text()),lambda image: [])
print(json.dumps({"resources":json.loads(files["manifests.json"]),"patches":json.loads(files["supervisor-patches.json"])}))`
	data, err := exec.Command("python3", "-c", code).CombinedOutput()
	if err != nil {
		t.Fatalf("render test fixture: %v: %s", err, data)
	}
	var result policyDocument
	if err := json.Unmarshal(data, &result); err != nil {
		t.Fatal(err)
	}
	return result
}

func asMap(value any) map[string]any { return value.(map[string]any) }
func asList(value any) []any         { return value.([]any) }

func clone(value map[string]any) map[string]any {
	data, _ := json.Marshal(value)
	var result map[string]any
	_ = json.Unmarshal(data, &result)
	return result
}

func admitted(policy map[string]any, pod map[string]any) (bool, error) {
	env, err := cel.NewEnv(cel.Variable("object", cel.DynType), cel.Variable("variables", cel.DynType))
	if err != nil {
		return false, err
	}
	variables := map[string]any{}
	activation := map[string]any{"object": pod, "variables": variables}
	evaluate := func(expression string) (any, error) {
		ast, issues := env.Compile(expression)
		if issues.Err() != nil {
			return nil, issues.Err()
		}
		program, err := env.Program(ast)
		if err != nil {
			return nil, err
		}
		value, _, err := program.Eval(activation)
		return value, err
	}
	spec := asMap(policy["spec"])
	for _, entry := range asList(spec["variables"]) {
		variable := asMap(entry)
		value, err := evaluate(variable["expression"].(string))
		if err != nil {
			return false, fmt.Errorf("variable %s: %w", variable["name"], err)
		}
		variables[variable["name"].(string)] = value
	}
	for _, entry := range asList(spec["validations"]) {
		value, err := evaluate(asMap(entry)["expression"].(string))
		if err != nil {
			return false, err
		}
		if value != types.True {
			return false, nil
		}
	}
	return true, nil
}

func TestSocketAdmissionBoundary(t *testing.T) {
	fixture := generated(t)
	var policy map[string]any
	var issuerPods []map[string]any
	for _, resource := range asList(fixture.Resources["items"]) {
		item := asMap(resource)
		switch item["kind"] {
		case "ValidatingAdmissionPolicy":
			if strings.HasSuffix(asMap(item["metadata"])["name"].(string), "-identity-sockets") {
				policy = item
			}
		case "StatefulSet", "DaemonSet":
			template := clone(asMap(asMap(item["spec"])["template"]))
			asMap(template["metadata"])["namespace"] = asMap(item["metadata"])["namespace"]
			issuerPods = append(issuerPods, template)
		}
	}
	for _, pod := range issuerPods {
		allowed, err := admitted(policy, pod)
		if err != nil || !allowed {
			t.Fatalf("generated issuer/agent pod refused: %v", err)
		}
	}
	for _, pod := range issuerPods {
		if asMap(asList(asMap(pod["spec"])["containers"])[0])["name"] != "agent" {
			continue
		}
		mutations := map[string]func(map[string]any){
			"privileged_agent": func(p map[string]any) {
				asMap(asMap(asList(asMap(p["spec"])["containers"])[0])["securityContext"])["privileged"] = true
			},
			"additional_host_mount": func(p map[string]any) {
				spec := asMap(p["spec"])
				spec["volumes"] = append(asList(spec["volumes"]), map[string]any{"name": "host-config", "hostPath": map[string]any{"path": "/etc/kubernetes", "type": "Directory"}})
			},
			"extra_capability": func(p map[string]any) {
				asMap(asMap(asMap(asList(asMap(p["spec"])["containers"])[0])["securityContext"])["capabilities"])["add"] = []any{"SYS_PTRACE", "SYS_ADMIN"}
			},
			"changed_entrypoint": func(p map[string]any) {
				asMap(asList(asMap(p["spec"])["containers"])[0])["command"] = []any{"/unexpected-entrypoint"}
			},
			"control_plane_node": func(p map[string]any) {
				asMap(p["spec"])["nodeName"] = "identity-control-a"
			},
			"unapproved_exec_hook": func(p map[string]any) {
				asMap(asList(asMap(p["spec"])["containers"])[0])["lifecycle"] = map[string]any{"postStart": map[string]any{"exec": map[string]any{"command": []any{"/unexpected-entrypoint"}}}}
			},
		}
		for name, mutate := range mutations {
			t.Run("agent/"+name, func(t *testing.T) {
				candidate := clone(pod)
				mutate(candidate)
				allowed, err := admitted(policy, candidate)
				if err != nil {
					t.Fatalf("policy evaluation error: %v", err)
				}
				if allowed {
					t.Fatal("unapproved node-agent privilege admitted")
				}
			})
		}
	}
	patch := fixture.Patches[0]
	base := map[string]any{"metadata": map[string]any{"namespace": patch["namespace"]}, "spec": patch["pod_spec_patch"]}
	allowed, err := admitted(policy, base)
	if err != nil || !allowed {
		t.Fatalf("admitted supervisor refused: %v", err)
	}
	ordinary := clone(base)
	asMap(ordinary["metadata"])["namespace"] = "untrusted-workloads"
	ordinarySpec := asMap(ordinary["spec"])
	ordinarySpec["serviceAccountName"] = "untrusted"
	delete(ordinarySpec, "volumes")
	delete(asMap(asList(ordinarySpec["containers"])[0]), "volumeMounts")
	allowed, err = admitted(policy, ordinary)
	if err != nil || !allowed {
		t.Fatalf("ordinary isolated workload refused: %v", err)
	}
	mutations := map[string]func(map[string]any){
		"var_run_alias_outside_supervisor_namespace": func(p map[string]any) {
			asMap(p["metadata"])["namespace"] = "untrusted-workloads"
			spec := asMap(p["spec"])
			spec["serviceAccountName"] = "untrusted"
			asMap(asMap(asList(spec["volumes"])[0])["hostPath"])["path"] = "/var/run/platform-spire/sockets"
		},
		"proc_alias_with_host_pid_outside_supervisor_namespace": func(p map[string]any) {
			asMap(p["metadata"])["namespace"] = "untrusted-workloads"
			spec := asMap(p["spec"])
			spec["serviceAccountName"] = "untrusted"
			spec["hostPID"] = true
			asMap(asMap(asList(spec["volumes"])[0])["hostPath"])["path"] = "/proc"
			security := asMap(asMap(asList(spec["containers"])[0])["securityContext"])
			security["runAsUser"] = 0
			security["runAsNonRoot"] = false
			security["privileged"] = true
		},
		"privileged_outside_supervisor_namespace": func(p map[string]any) {
			asMap(p["metadata"])["namespace"] = "untrusted-workloads"
			spec := asMap(p["spec"])
			spec["serviceAccountName"] = "untrusted"
			delete(spec, "volumes")
			container := asMap(asList(spec["containers"])[0])
			delete(container, "volumeMounts")
			asMap(container["securityContext"])["privileged"] = true
		},
		"wrong_namespace":       func(p map[string]any) { asMap(p["metadata"])["namespace"] = "another-tenant" },
		"wrong_service_account": func(p map[string]any) { asMap(p["spec"])["serviceAccountName"] = "untrusted" },
		"wrong_image":           func(p map[string]any) { asMap(asList(asMap(p["spec"])["containers"])[0])["image"] = "untrusted:latest" },
		"wrong_uid": func(p map[string]any) {
			asMap(asMap(asList(asMap(p["spec"])["containers"])[0])["securityContext"])["runAsUser"] = 0
		},
		"host_pid":   func(p map[string]any) { asMap(p["spec"])["hostPID"] = true },
		"root_mount": func(p map[string]any) { asMap(asMap(asList(asMap(p["spec"])["volumes"])[0])["hostPath"])["path"] = "/" },
		"ancestor_mount": func(p map[string]any) {
			asMap(asMap(asList(asMap(p["spec"])["volumes"])[0])["hostPath"])["path"] = "/run"
		},
		"writable_socket": func(p map[string]any) {
			asMap(asList(asMap(asList(asMap(p["spec"])["containers"])[0])["volumeMounts"])[0])["readOnly"] = false
		},
		"untrusted_container": func(p map[string]any) {
			spec := asMap(p["spec"])
			container := clone(asMap(asList(spec["containers"])[0]))
			container["name"] = "untrusted"
			spec["containers"] = append(asList(spec["containers"]), container)
		},
		"init_container": func(p map[string]any) { asMap(p["spec"])["initContainers"] = asList(asMap(p["spec"])["containers"]) },
		"ephemeral_container": func(p map[string]any) {
			asMap(p["spec"])["ephemeralContainers"] = asList(asMap(p["spec"])["containers"])
		},
		"privileged_sibling": func(p map[string]any) {
			spec := asMap(p["spec"])
			container := clone(asMap(asList(spec["containers"])[0]))
			container["name"] = "untrusted"
			delete(container, "volumeMounts")
			asMap(container["securityContext"])["privileged"] = true
			spec["containers"] = append(asList(spec["containers"]), container)
		},
		"ptrace_sibling": func(p map[string]any) {
			spec := asMap(p["spec"])
			container := clone(asMap(asList(spec["containers"])[0]))
			container["name"] = "untrusted"
			delete(container, "volumeMounts")
			asMap(asMap(container["securityContext"])["capabilities"])["add"] = []any{"SYS_PTRACE"}
			spec["containers"] = append(asList(spec["containers"]), container)
		},
	}
	for name, mutate := range mutations {
		t.Run(name, func(t *testing.T) {
			pod := clone(base)
			mutate(pod)
			allowed, err := admitted(policy, pod)
			if err != nil {
				t.Fatalf("policy evaluation error: %v", err)
			}
			if allowed {
				t.Fatal("unsafe identity endpoint mount admitted")
			}
		})
	}
}

func TestIdentityNamespaceBindingsStayOnApprovedNodes(t *testing.T) {
	fixture := generated(t)
	var policy map[string]any
	for _, resource := range asList(fixture.Resources["items"]) {
		item := asMap(resource)
		if item["kind"] == "ValidatingAdmissionPolicy" && strings.HasSuffix(asMap(item["metadata"])["name"].(string), "-identity-node-bindings") {
			policy = item
		}
	}
	if policy == nil {
		t.Fatal("protected identity namespaces have no node-binding admission guard")
	}
	for _, example := range []struct {
		namespace string
		node      string
		allow     bool
	}{
		{"identity-agents", "identity-node-a", true},
		{"identity-control", "identity-control-a", true},
		{"trusted-supervisors", "identity-node-b", true},
		{"identity-agents", "identity-control-a", false},
		{"identity-control", "identity-node-a", false},
		{"trusted-supervisors", "identity-control-a", false},
	} {
		t.Run(example.namespace+"/"+example.node, func(t *testing.T) {
			binding := map[string]any{"metadata": map[string]any{"namespace": example.namespace}, "target": map[string]any{"kind": "Node", "name": example.node}}
			allowed, err := admitted(policy, binding)
			if err != nil {
				t.Fatal(err)
			}
			if allowed != example.allow {
				t.Fatalf("node-binding verdict=%v, want %v", allowed, example.allow)
			}
		})
	}
}
