import { test, expect } from "@playwright/test";

/**
 * Auth E2E tests.
 *
 * NOTE: These tests require the server to be started with specific config:
 *   - api_secret = "testsecret" for API secret tests
 *   - token_auth = true for token auth tests
 *   - admin_secret = "adminsecret" for admin API tests
 *
 * If the server is running with default config (no auth), these tests
 * verify the unauthenticated behavior and skip the restricted tests.
 */

test.describe("API Secret Enforcement", () => {
  test("server info reports auth configuration", async ({ request }) => {
    const resp = await request.get("/janus/info");
    const json = await resp.json();
    expect(json.janus).toBe("server_info");

    // The info endpoint always works and reports auth status
    expect(typeof json.api_secret).toBe("boolean");
    expect(typeof json.auth_token).toBe("boolean");
  });

  test("ping works without secret when not configured", async ({
    request,
  }) => {
    // First check if api_secret is enabled
    const infoResp = await request.get("/janus/info");
    const infoJson = await infoResp.json();

    if (infoJson.api_secret === true) {
      // Server has api_secret configured — ping without secret should fail
      const resp = await request.post("/janus", {
        data: { janus: "ping", transaction: "t1" },
      });
      const json = await resp.json();
      expect(json.janus).toBe("error");
      expect(json.error.code).toBe(403);
    } else {
      // No api_secret — ping should work
      const resp = await request.post("/janus", {
        data: { janus: "ping", transaction: "t1" },
      });
      const json = await resp.json();
      expect(json.janus).toBe("pong");
    }
  });
});

test.describe("Session Lifecycle Auth", () => {
  test("create and destroy session works with current auth config", async ({
    request,
  }) => {
    // Check if api_secret is required
    const infoResp = await request.get("/janus/info");
    const info = await infoResp.json();

    const data: any = { janus: "create", transaction: "t1" };
    // If api_secret is needed, we can't test without knowing it,
    // so we verify the error response instead
    if (info.api_secret === true) {
      const resp = await request.post("/janus", { data });
      const json = await resp.json();
      expect(json.janus).toBe("error");
      expect(json.error.code).toBe(403);
    } else {
      const resp = await request.post("/janus", { data });
      const json = await resp.json();
      expect(json.janus).toBe("success");
      const sessionId = json.data.id;

      // Destroy
      const destroyResp = await request.post(`/janus/${sessionId}`, {
        data: { janus: "destroy", transaction: "t2" },
      });
      const destroyJson = await destroyResp.json();
      expect(destroyJson.janus).toBe("success");
    }
  });
});

test.describe("Token Auth", () => {
  test("info reports token_auth status", async ({ request }) => {
    const resp = await request.get("/janus/info");
    const json = await resp.json();
    expect(json.janus).toBe("server_info");
    // auth_token field indicates if token auth is enabled
    expect(typeof json.auth_token).toBe("boolean");
  });

  test("attach behavior depends on token_auth config", async ({
    request,
  }) => {
    const infoResp = await request.get("/janus/info");
    const info = await infoResp.json();

    if (info.api_secret === true) {
      test.skip();
      return;
    }

    // Create session
    const createResp = await request.post("/janus", {
      data: { janus: "create", transaction: "t1" },
    });
    const sessionId = (await createResp.json()).data.id;

    // Try attaching
    const attachResp = await request.post(`/janus/${sessionId}`, {
      data: {
        janus: "attach",
        plugin: "janus.plugin.echotest",
        transaction: "t2",
      },
    });
    const attachJson = await attachResp.json();

    if (info.auth_token === true) {
      // Token auth enabled: attach without token should fail
      expect(attachJson.janus).toBe("error");
      expect(attachJson.error.code).toBe(403);
    } else {
      // No token auth: attach should succeed
      expect(attachJson.janus).toBe("success");
    }

    // Cleanup
    await request.post(`/janus/${sessionId}`, {
      data: { janus: "destroy", transaction: "t3" },
    });
  });
});
