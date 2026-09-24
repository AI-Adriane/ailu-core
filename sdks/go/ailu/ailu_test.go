package ailu

import "testing"

func TestSmoke(t *testing.T) {
	if EngineVersion() == "" {
		t.Fatal("empty engine version")
	}

	components, err := ListComponentsJSON()
	if err != nil {
		t.Fatalf("list components: %v", err)
	}
	if !contains(components, "promptBuilder") {
		t.Fatalf("component catalog missing promptBuilder: %s", components)
	}

	output, err := RunComponentJSON(
		"promptBuilder",
		`{"template":"Hello {{name}}!","into":"prompt"}`,
		`{"name":"Ada"}`,
	)
	if err != nil {
		t.Fatalf("run component: %v", err)
	}
	if output != `{"prompt":"Hello Ada!"}` {
		t.Fatalf("unexpected component output: %s", output)
	}
}

func contains(value string, needle string) bool {
	for i := 0; i+len(needle) <= len(value); i++ {
		if value[i:i+len(needle)] == needle {
			return true
		}
	}
	return false
}
