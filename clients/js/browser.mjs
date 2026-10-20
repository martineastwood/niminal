import * as api from "./index.js";
export const { Client, NiminalError, PROTOCOL } = globalThis.niminal ?? api.default;
