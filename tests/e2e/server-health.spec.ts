import { test, expect } from "@playwright/test";

test.describe("Server Health", () => {
  test("GET /janus/info returns server_info", async ({ request }) => {
    const response = await request.get("/janus/info");
    expect(response.ok()).toBeTruthy();
    const json = await response.json();
    expect(json.janus).toBe("server_info");
    expect(json.server_name).toBeTruthy();
  });

  test("POST /janus with ping returns pong", async ({ request }) => {
    const response = await request.post("/janus", {
      data: { janus: "ping", transaction: "e2e-ping" },
    });
    expect(response.ok()).toBeTruthy();
    const json = await response.json();
    expect(json.janus).toBe("pong");
    expect(json.transaction).toBe("e2e-ping");
  });

  test("POST /janus create session returns session ID", async ({
    request,
  }) => {
    const response = await request.post("/janus", {
      data: { janus: "create", transaction: "e2e-create" },
    });
    expect(response.ok()).toBeTruthy();
    const json = await response.json();
    expect(json.janus).toBe("success");
    expect(json.data.id).toBeTruthy();
  });

  test("WebSocket ping/pong works", async ({ page }) => {
    // Use page.evaluate to test WebSocket connectivity
    const result = await page.evaluate(async () => {
      return new Promise<{ success: boolean; response: string }>(
        (resolve, reject) => {
          const ws = new WebSocket("ws://localhost:8188");
          ws.onopen = () => {
            ws.send(
              JSON.stringify({
                janus: "ping",
                transaction: "ws-e2e-ping",
              })
            );
          };
          ws.onmessage = (event) => {
            ws.close();
            resolve({ success: true, response: event.data });
          };
          ws.onerror = () => reject(new Error("WebSocket error"));
          setTimeout(() => reject(new Error("WebSocket timeout")), 5000);
        }
      );
    });

    expect(result.success).toBe(true);
    const json = JSON.parse(result.response);
    expect(json.janus).toBe("pong");
  });
});
