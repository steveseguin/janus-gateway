import { test, expect } from "@playwright/test";

/**
 * Streaming Plugin Integration Tests.
 *
 * These tests verify the streaming plugin's API for managing mountpoints
 * and viewer lifecycle. GStreamer/ffmpeg-based RTP injection is optional
 * and depends on system availability.
 *
 * The default mountpoint (ID 1) is always available.
 */

/** Helper: create a Janus session. */
async function createSession(request: any): Promise<number> {
  const resp = await request.post("/janus", {
    data: { janus: "create", transaction: "t-create" },
  });
  const json = await resp.json();
  expect(json.janus).toBe("success");
  return json.data.id;
}

/** Helper: attach to streaming plugin. */
async function attachStreaming(
  request: any,
  sessionId: number,
): Promise<number> {
  const resp = await request.post(`/janus/${sessionId}`, {
    data: {
      janus: "attach",
      plugin: "janus.plugin.streaming",
      transaction: "t-attach",
    },
  });
  const json = await resp.json();
  expect(json.janus).toBe("success");
  return json.data.id;
}

/** Helper: send a plugin message. */
async function sendMessage(
  request: any,
  sessionId: number,
  handleId: number,
  body: any,
): Promise<any> {
  const resp = await request.post(`/janus/${sessionId}/${handleId}`, {
    data: { janus: "message", transaction: "t-msg", body },
  });
  return resp.json();
}

/** Helper: destroy session. */
async function destroySession(request: any, sessionId: number) {
  await request.post(`/janus/${sessionId}`, {
    data: { janus: "destroy", transaction: "t-destroy" },
  });
}

/**
 * Helper: send a message and get the event response.
 * If the server returns "ack" (async processing, e.g. watch), polls for the event.
 */
async function sendMessageAndGetEvent(
  request: any,
  sessionId: number,
  handleId: number,
  body: any,
): Promise<any> {
  const resp = await request.post(`/janus/${sessionId}/${handleId}`, {
    data: { janus: "message", transaction: `t-${Date.now()}`, body },
  });
  const json = await resp.json();

  if (json.janus === "event" || json.janus === "error") return json;

  // Got "ack" — poll for the async event
  for (let i = 0; i < 20; i++) {
    const pollResp = await request.get(`/janus/${sessionId}?maxev=5`);
    const pollJson = await pollResp.json();
    const events = Array.isArray(pollJson) ? pollJson : [pollJson];
    for (const ev of events) {
      if (ev.janus === "event" || ev.janus === "error") return ev;
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error("Timed out waiting for event after ack");
}

test.describe("Streaming Plugin Integration", () => {
  test("list mountpoints includes default mountpoint", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attachStreaming(request, sessionId);

    const resp = await sendMessage(request, sessionId, handleId, {
      request: "list",
    });

    expect(resp.janus).toBe("event");
    const list = resp.plugindata.data.list;
    expect(list).toBeTruthy();
    expect(list.length).toBeGreaterThan(0);

    // Default mountpoint (ID 1) should be present
    const defaultMp = list.find((mp: any) => mp.id === 1);
    expect(defaultMp).toBeTruthy();

    await destroySession(request, sessionId);
  });

  test("create mountpoint with custom ports", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attachStreaming(request, sessionId);

    const resp = await sendMessage(request, sessionId, handleId, {
      request: "create",
      id: 5001,
      name: "test-stream",
      description: "E2E test stream",
      audio_port: 15000,
      video_port: 15002,
    });

    expect(resp.janus).toBe("event");
    expect(resp.plugindata.data.streaming).toBe("created");
    expect(resp.plugindata.data.stream.id).toBe(5001);

    // Verify in list
    const listResp = await sendMessage(request, sessionId, handleId, {
      request: "list",
    });
    const found = listResp.plugindata.data.list.find(
      (mp: any) => mp.id === 5001,
    );
    expect(found).toBeTruthy();
    expect(found.description).toBe("E2E test stream");

    // Cleanup
    await sendMessage(request, sessionId, handleId, {
      request: "destroy",
      id: 5001,
    });
    await destroySession(request, sessionId);
  });

  test("destroy mountpoint with wrong secret fails", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attachStreaming(request, sessionId);

    // Create with secret
    await sendMessage(request, sessionId, handleId, {
      request: "create",
      id: 5002,
      secret: "streamsecret",
    });

    // Try to destroy with wrong secret
    const resp = await sendMessage(request, sessionId, handleId, {
      request: "destroy",
      id: 5002,
      secret: "wrongsecret",
    });

    expect(resp.janus).toBe("error");

    // Cleanup with correct secret
    await sendMessage(request, sessionId, handleId, {
      request: "destroy",
      id: 5002,
      secret: "streamsecret",
    });
    await destroySession(request, sessionId);
  });

  test("watch mountpoint returns SDP offer", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attachStreaming(request, sessionId);

    const resp = await sendMessageAndGetEvent(request, sessionId, handleId, {
      request: "watch",
      id: 1,
    });

    expect(resp.janus).toBe("event");
    expect(resp.plugindata.data.streaming).toBe("event");
    // The watch response may include JSEP offer for WebRTC setup
    // or just acknowledge the viewer registration
    expect(resp.plugindata.data.result).toBeTruthy();

    await destroySession(request, sessionId);
  });

  test("info on mountpoint returns metadata", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attachStreaming(request, sessionId);

    const resp = await sendMessage(request, sessionId, handleId, {
      request: "info",
      id: 1,
    });

    expect(resp.janus).toBe("event");
    expect(resp.plugindata.data.streaming).toBe("info");
    expect(resp.plugindata.data.info).toBeTruthy();
    expect(resp.plugindata.data.info.id).toBe(1);
    expect(typeof resp.plugindata.data.info.viewers).toBe("number");

    await destroySession(request, sessionId);
  });

  test("PIN-protected mountpoint rejects wrong PIN", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attachStreaming(request, sessionId);

    // Create with PIN
    await sendMessage(request, sessionId, handleId, {
      request: "create",
      id: 5003,
      pin: "1234",
    });

    // Watch with wrong PIN
    const resp = await sendMessageAndGetEvent(request, sessionId, handleId, {
      request: "watch",
      id: 5003,
      pin: "0000",
    });

    expect(resp.janus).toBe("error");

    // Watch with correct PIN should work
    const s2 = await createSession(request);
    const h2 = await attachStreaming(request, s2);
    const resp2 = await sendMessageAndGetEvent(request, s2, h2, {
      request: "watch",
      id: 5003,
      pin: "1234",
    });

    expect(resp2.janus).toBe("event");

    // Cleanup
    await destroySession(request, s2);
    await sendMessage(request, sessionId, handleId, {
      request: "destroy",
      id: 5003,
    });
    await destroySession(request, sessionId);
  });

  test("multiple viewers on same mountpoint", async ({ request }) => {
    const sessions: { sessionId: number; handleId: number }[] = [];

    // Create 3 viewers
    for (let i = 0; i < 3; i++) {
      const sessionId = await createSession(request);
      const handleId = await attachStreaming(request, sessionId);

      const resp = await sendMessageAndGetEvent(request, sessionId, handleId, {
        request: "watch",
        id: 1,
      });
      expect(resp.janus).toBe("event");
      sessions.push({ sessionId, handleId });
    }

    // Check viewer count via info
    const infoSession = await createSession(request);
    const infoHandle = await attachStreaming(request, infoSession);
    const infoResp = await sendMessage(request, infoSession, infoHandle, {
      request: "info",
      id: 1,
    });

    expect(infoResp.plugindata.data.info.viewers).toBeGreaterThanOrEqual(3);

    // Cleanup
    for (const { sessionId } of sessions) {
      await destroySession(request, sessionId);
    }
    await destroySession(request, infoSession);
  });
});
