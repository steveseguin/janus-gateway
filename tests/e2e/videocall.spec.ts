import { test, expect } from "@playwright/test";

/**
 * VideoCall plugin E2E tests.
 *
 * Tests the VideoCall plugin's register, list, call, and hangup flows
 * via the Janus HTTP API.
 */

async function createSessionAndAttach(
  request: any,
): Promise<{ sessionId: number; handleId: number }> {
  const createResp = await request.post("/janus", {
    data: { janus: "create", transaction: "vc-create" },
  });
  const sessionId = (await createResp.json()).data.id;

  const attachResp = await request.post(`/janus/${sessionId}`, {
    data: {
      janus: "attach",
      plugin: "janus.plugin.videocall",
      transaction: "vc-attach",
    },
  });
  const attachJson = await attachResp.json();
  expect(attachJson.janus).toBe("success");
  const handleId = attachJson.data.id;

  return { sessionId, handleId };
}

async function sendMessage(
  request: any,
  sessionId: number,
  handleId: number,
  body: any,
  jsep?: any,
): Promise<any> {
  const data: any = {
    janus: "message",
    transaction: `vc-${Date.now()}`,
    body,
  };
  if (jsep) {
    data.jsep = jsep;
  }
  const resp = await request.post(`/janus/${sessionId}/${handleId}`, {
    data,
  });
  return resp.json();
}

async function destroySession(request: any, sessionId: number) {
  await request.post(`/janus/${sessionId}`, {
    data: { janus: "destroy", transaction: "vc-destroy" },
  });
}

test.describe("VideoCall Plugin", () => {
  test("register two users and list shows both", async ({ request }) => {
    const alice = await createSessionAndAttach(request);
    const bob = await createSessionAndAttach(request);

    // Register Alice
    const regAlice = await sendMessage(
      request,
      alice.sessionId,
      alice.handleId,
      { request: "register", username: "alice-e2e" },
    );
    expect(regAlice.janus).toBe("event");
    expect(regAlice.plugindata.data.result.event).toBe("registered");
    expect(regAlice.plugindata.data.result.username).toBe("alice-e2e");

    // Register Bob
    const regBob = await sendMessage(
      request,
      bob.sessionId,
      bob.handleId,
      { request: "register", username: "bob-e2e" },
    );
    expect(regBob.janus).toBe("event");
    expect(regBob.plugindata.data.result.event).toBe("registered");

    // List usernames
    const listResp = await sendMessage(
      request,
      alice.sessionId,
      alice.handleId,
      { request: "list" },
    );
    expect(listResp.janus).toBe("event");
    const list = listResp.plugindata.data.result.list;
    expect(list).toContain("alice-e2e");
    expect(list).toContain("bob-e2e");

    // Cleanup
    await destroySession(request, alice.sessionId);
    await destroySession(request, bob.sessionId);
  });

  test("register duplicate username returns error", async ({ request }) => {
    const alice = await createSessionAndAttach(request);
    const bob = await createSessionAndAttach(request);

    // Register Alice
    await sendMessage(request, alice.sessionId, alice.handleId, {
      request: "register",
      username: "dup-user-e2e",
    });

    // Bob tries same name
    const resp = await sendMessage(request, bob.sessionId, bob.handleId, {
      request: "register",
      username: "dup-user-e2e",
    });
    expect(resp.janus).toBe("event");
    expect(resp.plugindata.data.error_code).toBe(476);

    // Cleanup
    await destroySession(request, alice.sessionId);
    await destroySession(request, bob.sessionId);
  });

  test("call nonexistent user returns error", async ({ request }) => {
    const alice = await createSessionAndAttach(request);

    // Register Alice
    await sendMessage(request, alice.sessionId, alice.handleId, {
      request: "register",
      username: "alice-call-e2e",
    });

    // Call nonexistent user
    const resp = await sendMessage(
      request,
      alice.sessionId,
      alice.handleId,
      { request: "call", username: "nobody-e2e" },
      { type: "offer", sdp: "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n" },
    );
    expect(resp.janus).toBe("event");
    expect(resp.plugindata.data.error_code).toBe(478);

    // Cleanup
    await destroySession(request, alice.sessionId);
  });

  test("call lifecycle: register, call, hangup", async ({ request }) => {
    const alice = await createSessionAndAttach(request);
    const bob = await createSessionAndAttach(request);

    // Register both
    await sendMessage(request, alice.sessionId, alice.handleId, {
      request: "register",
      username: "alice-lifecycle-e2e",
    });
    await sendMessage(request, bob.sessionId, bob.handleId, {
      request: "register",
      username: "bob-lifecycle-e2e",
    });

    // Alice calls Bob
    const callResp = await sendMessage(
      request,
      alice.sessionId,
      alice.handleId,
      { request: "call", username: "bob-lifecycle-e2e" },
      { type: "offer", sdp: "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n" },
    );
    expect(callResp.janus).toBe("event");
    expect(callResp.plugindata.data.result.event).toBe("calling");

    // Alice hangs up
    const hangupResp = await sendMessage(
      request,
      alice.sessionId,
      alice.handleId,
      { request: "hangup" },
    );
    expect(hangupResp.janus).toBe("event");
    expect(hangupResp.plugindata.data.result.event).toBe("hangup");

    // Cleanup
    await destroySession(request, alice.sessionId);
    await destroySession(request, bob.sessionId);
  });

  test("call without register returns error", async ({ request }) => {
    const alice = await createSessionAndAttach(request);

    // Try calling without registering
    const resp = await sendMessage(
      request,
      alice.sessionId,
      alice.handleId,
      { request: "call", username: "someone" },
      { type: "offer", sdp: "v=0\r\n" },
    );
    expect(resp.janus).toBe("event");
    expect(resp.plugindata.data.error_code).toBe(473);

    // Cleanup
    await destroySession(request, alice.sessionId);
  });
});
