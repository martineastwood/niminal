import * as api from "./index.js";
export const { Client, LiveClient, NiminalError, PROTOCOL } = globalThis.niminal ?? api.default;
