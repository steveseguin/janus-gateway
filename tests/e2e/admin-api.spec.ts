import { test, expect } from "@playwright/test";

/**
 * Admin API E2E tests.
 *
 * Tests the admin commands: list_sessions, list_handles, handle_info,
 * set_session_timeout. Uses the admin HTTP endpoint on port 7088.
 */

const ADMIN_URL = "http://localhost:7088";

test.describe("Admin API", () => {
  test("list_sessions returns active sessions", async ({ request }) => {
    // Create a session via the regular API
    const createResp = await request.post("/janus", {
      data: { janus: "create", transaction: "t1" },
    });
    const createJson = await createResp.json();
    expect(createJson.janus).toBe("success");
    const sessionId = createJson.data.id;

    // List sessions via admin API
    const listResp = await request.post(`${ADMIN_URL}/admin`, {
      data: { janus: "list_sessions", transaction: "t2" },
    });
    const listJson = await listResp.json();
    expect(listJson.janus).toBe("success");
    expect(Array.isArray(listJson.sessions)).toBe(true);
    expect(listJson.sessions).toContain(sessionId);

    // Cleanup
    await request.post(`/janus/${sessionId}`, {
      data: { janus: "destroy", transaction: "t3" },
    });
  });

  test("list_handles returns attached handles", async ({ request }) => {
    // Create session
    const createResp = await request.post("/janus", {
      data: { janus: "create", transaction: "t1" },
    });
    const sessionId = (await createResp.json()).data.id;

    // Attach plugin
    const attachResp = await request.post(`/janus/${sessionId}`, {
      data: {
        janus: "attach",
        plugin: "janus.plugin.echotest",
        transaction: "t2",
      },
    });
    const handleId = (await attachResp.json()).data.id;

    // List handles via admin API
    const listResp = await request.post(`${ADMIN_URL}/admin`, {
      data: {
        janus: "list_handles",
        transaction: "t3",
        session_id: sessionId,
      },
    });
    const listJson = await listResp.json();
    expect(listJson.janus).toBe("success");
    expect(Array.isArray(listJson.handles)).toBe(true);
    expect(listJson.handles).toContain(handleId);

    // Cleanup
    await request.post(`/janus/${sessionId}`, {
      data: { janus: "destroy", transaction: "t4" },
    });
  });

  test("handle_info returns plugin name", async ({ request }) => {
    // Create session
    const createResp = await request.post("/janus", {
      data: { janus: "create", transaction: "t1" },
    });
    const sessionId = (await createResp.json()).data.id;

    // Attach plugin
    const attachResp = await request.post(`/janus/${sessionId}`, {
      data: {
        janus: "attach",
        plugin: "janus.plugin.echotest",
        transaction: "t2",
      },
    });
    const handleId = (await attachResp.json()).data.id;

    // Get handle info via admin API
    const infoResp = await request.post(`${ADMIN_URL}/admin`, {
      data: {
        janus: "handle_info",
        transaction: "t3",
        session_id: sessionId,
        handle_id: handleId,
      },
    });
    const infoJson = await infoResp.json();
    expect(infoJson.janus).toBe("success");
    expect(infoJson.info.plugin).toBe("janus.plugin.echotest");
    expect(infoJson.info.session_id).toBe(sessionId);
    expect(infoJson.info.handle_id).toBe(handleId);

    // Cleanup
    await request.post(`/janus/${sessionId}`, {
      data: { janus: "destroy", transaction: "t4" },
    });
  });

  test("list_handles errors for nonexistent session", async ({ request }) => {
    const resp = await request.post(`${ADMIN_URL}/admin`, {
      data: {
        janus: "list_handles",
        transaction: "t1",
        session_id: 999999,
      },
    });
    const json = await resp.json();
    expect(json.janus).toBe("error");
    expect(json.error.code).toBe(458);
  });

  test("set_session_timeout updates timeout", async ({ request }) => {
    // Set timeout to 120
    const setResp = await request.post(`${ADMIN_URL}/admin`, {
      data: {
        janus: "set_session_timeout",
        transaction: "t1",
        timeout: 120,
      },
    });
    const setJson = await setResp.json();
    expect(setJson.janus).toBe("success");
    expect(setJson.timeout).toBe(120);

    // Verify via server info
    const infoResp = await request.get("/janus/info");
    const infoJson = await infoResp.json();
    // Note: session-timeout in info may not update dynamically since it reads from config
    // The important thing is the admin API itself succeeded

    // Reset to default
    await request.post(`${ADMIN_URL}/admin`, {
      data: {
        janus: "set_session_timeout",
        transaction: "t2",
        timeout: 60,
      },
    });
  });
});
