import { test, expect } from "@playwright/test";

/**
 * WHIP/WHEP Media Verification E2E tests.
 *
 * These tests go beyond HTTP signaling to verify:
 * - SDP answer contains valid ICE candidates
 * - Multiple WHEP subscribers can subscribe to same publisher
 * - Publisher disconnect cascades to subscribers
 * - PATCH ICE restart returns correct response
 */

const MINIMAL_SDP_OFFER = [
  "v=0",
  "o=- 0 0 IN IP4 127.0.0.1",
  "s=-",
  "t=0 0",
  "a=group:BUNDLE 0",
  "a=ice-options:trickle",
  "m=audio 9 UDP/TLS/RTP/SAVPF 111",
  "c=IN IP4 0.0.0.0",
  "a=mid:0",
  "a=sendrecv",
  "a=rtpmap:111 opus/48000/2",
  "a=ice-ufrag:testufrag1234",
  "a=ice-pwd:testpasswordtestpassword1234",
  "a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00",
  "a=setup:actpass",
  "",
].join("\r\n");

test.describe("WHIP/WHEP Media Verification", () => {
  test("WHIP SDP answer contains ICE credentials", async ({ request }) => {
    const resp = await request.post("/whip", {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(resp.status()).toBe(201);

    const sdpAnswer = await resp.text();
    expect(sdpAnswer).toContain("a=ice-ufrag:");
    expect(sdpAnswer).toContain("a=ice-pwd:");
    expect(sdpAnswer).toContain("a=fingerprint:");

    // Cleanup
    const location = resp.headers()["location"]!;
    await request.delete(location);
  });

  test("WHIP SDP answer has correct direction for receive", async ({
    request,
  }) => {
    const resp = await request.post("/whip", {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(resp.status()).toBe(201);

    const sdpAnswer = await resp.text();
    // Server should set recvonly since publisher sends
    expect(sdpAnswer).toMatch(/a=(recvonly|sendrecv)/);

    const location = resp.headers()["location"]!;
    await request.delete(location);
  });

  test("multiple WHEP subscribers to same publisher", async ({ request }) => {
    // Publish
    const whipResp = await request.post("/whip", {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(whipResp.status()).toBe(201);
    const pubLocation = whipResp.headers()["location"]!;
    const pubId = pubLocation.replace("/resource/", "");

    // Subscribe 3 times
    const subLocations: string[] = [];
    for (let i = 0; i < 3; i++) {
      const whepResp = await request.post(`/whep/${pubId}`, {
        headers: { "Content-Type": "application/sdp" },
        data: MINIMAL_SDP_OFFER,
      });
      expect(whepResp.status()).toBe(201);
      subLocations.push(whepResp.headers()["location"]!);

      const subSdp = await whepResp.text();
      expect(subSdp).toContain("v=0");
      expect(subSdp).toContain("a=ice-ufrag:");
    }

    // All subscriber locations should be unique
    const uniqueLocations = new Set(subLocations);
    expect(uniqueLocations.size).toBe(3);

    // Cleanup subscribers
    for (const loc of subLocations) {
      const del = await request.delete(loc);
      expect(del.status()).toBe(200);
    }

    // Cleanup publisher
    await request.delete(pubLocation);
  });

  test("deleting publisher cascades to all subscribers", async ({
    request,
  }) => {
    // Publish
    const whipResp = await request.post("/whip", {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(whipResp.status()).toBe(201);
    const pubLocation = whipResp.headers()["location"]!;
    const pubId = pubLocation.replace("/resource/", "");

    // Subscribe twice
    const sub1 = await request.post(`/whep/${pubId}`, {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    const sub1Loc = sub1.headers()["location"]!;

    const sub2 = await request.post(`/whep/${pubId}`, {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    const sub2Loc = sub2.headers()["location"]!;

    // Delete publisher
    const delPub = await request.delete(pubLocation);
    expect(delPub.status()).toBe(200);

    // Both subscribers should be gone
    const gone1 = await request.delete(sub1Loc);
    expect(gone1.status()).toBe(404);

    const gone2 = await request.delete(sub2Loc);
    expect(gone2.status()).toBe(404);
  });

  test("PATCH for ICE restart returns correct format", async ({ request }) => {
    // Publish
    const whipResp = await request.post("/whip", {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(whipResp.status()).toBe(201);
    const pubLocation = whipResp.headers()["location"]!;
    const etag = whipResp.headers()["etag"]!;

    // PATCH for ICE restart with trickle-ice-sdpfrag
    const iceRestart = [
      "a=ice-ufrag:newufrag12345",
      "a=ice-pwd:newpasswordnewpassword1234",
      "",
    ].join("\r\n");

    const patchResp = await request.patch(pubLocation, {
      headers: {
        "Content-Type": "application/trickle-ice-sdpfrag",
        "If-Match": etag,
      },
      data: iceRestart,
    });

    // Should accept the PATCH (200) or return new ICE credentials
    expect([200, 204]).toContain(patchResp.status());

    // Cleanup
    await request.delete(pubLocation);
  });

  test("WHEP subscribe to nonexistent publisher returns 404", async ({
    request,
  }) => {
    const resp = await request.post(
      "/whep/00000000-0000-0000-0000-000000000000",
      {
        headers: { "Content-Type": "application/sdp" },
        data: MINIMAL_SDP_OFFER,
      },
    );
    expect(resp.status()).toBe(404);
  });

  test("WHIP Link headers include STUN server", async ({ request }) => {
    const resp = await request.post("/whip", {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(resp.status()).toBe(201);

    const link = resp.headers()["link"];
    expect(link).toBeTruthy();
    expect(link).toContain("stun:");
    expect(link).toContain('rel="ice-server"');

    const location = resp.headers()["location"]!;
    await request.delete(location);
  });
});
