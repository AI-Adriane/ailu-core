package ai.ailu;

import java.util.ArrayList;
import java.util.List;

public final class AiluSmoke {
  private AiluSmoke() {}

  public static void main(String[] args) {
    if (Ailu.engineVersion().isBlank()) {
      throw new IllegalStateException("empty engine version");
    }

    String components = Ailu.listComponentsJson();
    if (!components.contains("promptBuilder")) {
      throw new IllegalStateException("component catalog missing promptBuilder: " + components);
    }

    String output = Ailu.runComponentJson(
        "promptBuilder",
        "{\"template\":\"Hello {{name}}!\",\"into\":\"prompt\"}",
        "{\"name\":\"Ada\"}"
    );
    if (!"{\"prompt\":\"Hello Ada!\"}".equals(output)) {
      throw new IllegalStateException("unexpected component output: " + output);
    }

    List<String> events = new ArrayList<>();
    Ailu.AiluCallbacks callbacks = new Ailu.AiluCallbacks(
        (payload, userData) -> Ailu.AiluCallbackResult.ok("{\"greeting\":\"hello from java\"}"),
        (payload, userData) -> Ailu.AiluCallbackResult.ok("true"),
        (payload, userData) -> events.add(payload)
    );
    String run = Ailu.engineRunJson("""
        {
          "graph": {
            "id": "java-callback",
            "version": "1.0.0",
            "name": "Java callback",
            "entryNodeId": "start",
            "channels": {
              "greeting": { "type": "string", "reducer": "replace" }
            },
            "nodes": [
              { "id": "start", "type": "action", "label": "Start" }
            ],
            "edges": []
          },
          "runId": "run-java",
          "jsNodeIds": ["start"]
        }
        """, callbacks);
    if (!run.contains("hello from java")) {
      throw new IllegalStateException("unexpected callback run output: " + run + " events=" + events);
    }

    System.out.println("java ok");
  }
}
