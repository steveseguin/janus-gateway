import { test, expect } from "@playwright/test";

test.describe("EchoTest Plugin", () => {
  test("create session and attach to echotest plugin", async ({ request }) => {
    // Create session
    const createResp = await request.post("/janus", {
      data: { janus: "create", transaction: "t1" },
    });
    const createJson = await createResp.json();
    expect(createJson.janus).toBe("success");
    const sessionId = createJson.data.id;

    // Attach to echotest plugin
    const attachResp = await request.post(`/janus/${sessionId}`, {
      data: {
        janus: "attach",
        plugin: "janus.plugin.echotest",
        transaction: "t2",
      },
    });
    const attachJson = await attachResp.json();
    expect(attachJson.janus).toBe("success");
    expect(attachJson.data.id).toBeTruthy();
    const handleId = attachJson.data.id;

    // Send a message to echotest
    const msgResp = await request.post(`/janus/${sessionId}/${handleId}`, {
      data: {
        janus: "message",
        transaction: "t3",
        body: { audio: true, video: true },
      },
    });
    const msgJson = await msgResp.json();
    // Should get ack or success
    expect(["ack", "success", "event"]).toContain(msgJson.janus);

    // Detach
    const detachResp = await request.post(`/janus/${sessionId}/${handleId}`, {
      data: { janus: "detach", transaction: "t4" },
    });
    const detachJson = await detachResp.json();
    expect(detachJson.janus).toBe("success");

    // Destroy session
    const destroyResp = await request.post(`/janus/${sessionId}`, {
      data: { janus: "destroy", transaction: "t5" },
    });
    const destroyJson = await destroyResp.json();
    expect(destroyJson.janus).toBe("success");
  });
});
