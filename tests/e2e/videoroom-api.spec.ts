import { test, expect } from "@playwright/test";

/** Helper: create a Janus session and return its ID. */
async function createSession(request: any): Promise<number> {
  const resp = await request.post("/janus", {
    data: { janus: "create", transaction: "t-create" },
  });
  const json = await resp.json();
  expect(json.janus).toBe("success");
  return json.data.id;
}

/** Helper: attach to a plugin and return the handle ID. */
async function attach(
  request: any,
  sessionId: number,
  plugin: string,
): Promise<number> {
  const resp = await request.post(`/janus/${sessionId}`, {
    data: { janus: "attach", plugin, transaction: "t-attach" },
  });
  const json = await resp.json();
  expect(json.janus).toBe("success");
  return json.data.id;
}

/** Helper: send a plugin message and return the response JSON. */
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

/** Helper: cleanup a session. */
async function destroySession(request: any, sessionId: number) {
  await request.post(`/janus/${sessionId}`, {
    data: { janus: "destroy", transaction: "t-destroy" },
  });
}

test.describe("VideoRoom API Comprehensive", () => {
  test("create room with all options", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attach(request, sessionId, "janus.plugin.videoroom");

    const resp = await sendMessage(request, sessionId, handleId, {
      request: "create",
      room: 9001,
      description: "Full Options Room",
      max_publishers: 3,
      bitrate: 512000,
      pin: "mypin",
      secret: "mysecret",
      is_private: true,
    });

    expect(resp.janus).toBe("event");
    expect(resp.plugindata.data.videoroom).toBe("created");
    expect(resp.plugindata.data.room).toBe(9001);

    // Verify room is not listed (private)
    const listResp = await sendMessage(request, sessionId, handleId, {
      request: "list",
    });
    const rooms = listResp.plugindata?.data?.list || [];
    const found = rooms.find((r: any) => r.room === 9001);
    expect(found).toBeFalsy();

    // Verify room exists
    const existsResp = await sendMessage(request, sessionId, handleId, {
      request: "exists",
      room: 9001,
    });
    expect(existsResp.plugindata.data.exists).toBe(true);

    await destroySession(request, sessionId);
  });

  test("join with wrong PIN returns error", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attach(request, sessionId, "janus.plugin.videoroom");

    // Create PIN-protected room
    await sendMessage(request, sessionId, handleId, {
      request: "create",
      room: 9002,
      pin: "correctpin",
    });

    // Join with wrong PIN
    const joinResp = await sendMessage(request, sessionId, handleId, {
      request: "join",
      room: 9002,
      ptype: "publisher",
      pin: "wrongpin",
    });

    expect(joinResp.janus).toBe("error");

    await destroySession(request, sessionId);
  });

  test("join with correct PIN succeeds", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attach(request, sessionId, "janus.plugin.videoroom");

    await sendMessage(request, sessionId, handleId, {
      request: "create",
      room: 9003,
      pin: "correctpin",
    });

    const joinResp = await sendMessage(request, sessionId, handleId, {
      request: "join",
      room: 9003,
      ptype: "publisher",
      pin: "correctpin",
    });

    expect(joinResp.janus).toBe("event");
    expect(joinResp.plugindata.data.videoroom).toBe("joined");

    await destroySession(request, sessionId);
  });

  test("exceed max_publishers returns error", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attach(request, sessionId, "janus.plugin.videoroom");

    // Create room with max 1 publisher
    await sendMessage(request, sessionId, handleId, {
      request: "create",
      room: 9004,
      max_publishers: 1,
    });

    // Join first publisher
    const join1 = await sendMessage(request, sessionId, handleId, {
      request: "join",
      room: 9004,
      ptype: "publisher",
    });
    expect(join1.plugindata.data.videoroom).toBe("joined");

    // Create second session and try to join
    const sessionId2 = await createSession(request);
    const handleId2 = await attach(
      request,
      sessionId2,
      "janus.plugin.videoroom",
    );
    const join2 = await sendMessage(request, sessionId2, handleId2, {
      request: "join",
      room: 9004,
      ptype: "publisher",
    });

    // Should be error (room full)
    expect(join2.janus).toBe("error");

    await destroySession(request, sessionId);
    await destroySession(request, sessionId2);
  });

  test("destroy room with wrong secret fails", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attach(request, sessionId, "janus.plugin.videoroom");

    await sendMessage(request, sessionId, handleId, {
      request: "create",
      room: 9005,
      secret: "admin123",
    });

    const destroyResp = await sendMessage(request, sessionId, handleId, {
      request: "destroy",
      room: 9005,
      secret: "wrong",
    });

    expect(destroyResp.janus).toBe("error");

    // Room should still exist
    const existsResp = await sendMessage(request, sessionId, handleId, {
      request: "exists",
      room: 9005,
    });
    expect(existsResp.plugindata.data.exists).toBe(true);

    await destroySession(request, sessionId);
  });

  test("destroy room with correct secret succeeds", async ({ request }) => {
    const sessionId = await createSession(request);
    const handleId = await attach(request, sessionId, "janus.plugin.videoroom");

    await sendMessage(request, sessionId, handleId, {
      request: "create",
      room: 9006,
      secret: "admin123",
    });

    const destroyResp = await sendMessage(request, sessionId, handleId, {
      request: "destroy",
      room: 9006,
      secret: "admin123",
    });

    expect(destroyResp.plugindata.data.videoroom).toBe("destroyed");

    await destroySession(request, sessionId);
  });

  test("listparticipants returns display names and IDs", async ({
    request,
  }) => {
    const sessionId = await createSession(request);
    const handleId = await attach(request, sessionId, "janus.plugin.videoroom");

    // Join default room as publisher with display name
    const joinResp = await sendMessage(request, sessionId, handleId, {
      request: "join",
      room: 1234,
      ptype: "publisher",
      display: "E2E-Alice",
    });

    expect(joinResp.plugindata.data.videoroom).toBe("joined");
    const userId = joinResp.plugindata.data.id;
    expect(userId).toBeTruthy();

    // List participants
    const listResp = await sendMessage(request, sessionId, handleId, {
      request: "listparticipants",
      room: 1234,
    });

    const participants = listResp.plugindata?.data?.participants || [];
    expect(participants.length).toBeGreaterThan(0);

    const alice = participants.find((p: any) => p.id === userId);
    expect(alice).toBeTruthy();
    expect(alice.display).toBe("E2E-Alice");

    await destroySession(request, sessionId);
  });
});
