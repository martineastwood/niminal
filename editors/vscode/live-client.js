// The client of `nimlive live`, the sequencer this extension starts: it speaks nimlive's own API
// (clips, scenes, quantized edits) over the generic connection that niminal-client provides.
const { Client } = require("niminal-client");

const withQuantize = (params, quantize) => (quantize ? { ...params, quantize } : params);

class LiveClient extends Client {
  constructor(options = {}) {
    super({ port: 7400, ...options });
  }

  eval(source, { quantize, dir } = {}) {
    return this.call("eval", { source, ...(quantize ? { quantize } : {}), ...(dir ? { dir } : {}) });
  }

  defineClip(clip, { quantize } = {}) {
    return this.call("clip.define", withQuantize(clip, quantize));
  }

  defineScene(scene, { quantize } = {}) {
    return this.call("scene.define", withQuantize(scene, quantize));
  }

  launch(target, { quantize, then } = {}) {
    return this.call("launch", withQuantize({ target, ...(then ? { then } : {}) }, quantize)); // then: { target, after: "4bars" }
  }

  stop(target, { quantize } = {}) {
    return this.call("stop", withQuantize({ target }, quantize));
  }

  mute(target, { quantize } = {}) {
    return this.call("mute", withQuantize({ target }, quantize));
  }

  unmute(target, { quantize } = {}) {
    return this.call("unmute", withQuantize({ target }, quantize));
  }

  solo(target, { quantize } = {}) {
    return this.call("solo", withQuantize({ target }, quantize));
  }

  unsolo(target, { quantize } = {}) {
    return this.call("unsolo", withQuantize(target ? { target } : {}, quantize));
  }

  edit(track, change, { quantize } = {}) {
    return this.call("edit", withQuantize({ track, change }, quantize));
  }

  hush() {
    return this.call("hush");
  }
}

module.exports = { LiveClient };
