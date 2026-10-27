import Sandbox from "postman-sandbox";
import PostmanCollection from "postman-collection";

const { Event } = PostmanCollection;
let sandboxPromise;

function getSandbox() {
  if (!sandboxPromise) {
    sandboxPromise = new Promise((resolve, reject) => {
      Sandbox.createContext((error, context) => error ? reject(error) : resolve(context));
    });
  }
  return sandboxPromise;
}

function toPostmanBody(request) {
  if (request.bodyType === "form") {
    return {
      mode: "urlencoded",
      urlencoded: request.formBody.map((item) => ({
        key: item.key,
        value: item.value,
        disabled: !item.enabled,
        type: "text",
      })),
    };
  }
  if (request.bodyType === "formdata") {
    return {
      mode: "formdata",
      formdata: request.formData.map((item) => ({
        key: item.key,
        value: item.value,
        disabled: !item.enabled,
        type: item.type || "text",
      })),
    };
  }
  if (request.bodyType === "none") return undefined;
  return {
    mode: "raw",
    raw: request.body,
    options: { raw: { language: request.bodyType === "json" ? "json" : "text" } },
  };
}

function toPostmanRequest(request) {
  return {
    method: request.method,
    url: request.url,
    header: request.headers.map((item) => ({
      key: item.key,
      value: item.value,
      disabled: !item.enabled,
    })),
    body: toPostmanBody(request),
  };
}

function fromPostmanRequest(result, original) {
  const request = result.request || {};
  const body = request.body || {};
  const output = {
    ...original,
    method: request.method || original.method,
    url: new PostmanCollection.Url(request.url).toString(),
    headers: (request.header || []).map((item) => ({
      id: crypto.randomUUID(),
      key: item.key || "",
      value: item.value || "",
      enabled: !item.disabled,
    })),
  };

  if (body.mode === "urlencoded") {
    output.bodyType = "form";
    output.formBody = (body.urlencoded || []).map((item) => ({
      id: crypto.randomUUID(),
      key: item.key || "",
      value: item.value ?? "",
      enabled: !item.disabled,
    }));
  } else if (body.mode === "formdata") {
    output.bodyType = "formdata";
    output.formData = (body.formdata || []).map((item) => ({
      id: crypto.randomUUID(),
      key: item.key || "",
      value: item.value ?? "",
      enabled: !item.disabled,
      type: item.type || "text",
    }));
  } else if (body.mode === "raw") {
    output.body = body.raw || "";
  }
  return output;
}

function environmentValues(variables) {
  return variables
    .filter((item) => item.enabled && item.key)
    .map((item) => ({ key: item.key, value: item.value, type: "string", enabled: true }));
}

function environmentFromResult(result) {
  return (result.environment?.values || []).map((item) => ({
    id: crypto.randomUUID(),
    key: item.key,
    value: item.value ?? "",
    enabled: item.enabled !== false,
  })).map((item) => {
    const segments = item.key.split(".");
    if (segments.length > 3 || segments.some((segment) => !segment)) {
      throw new Error(`Environment variable name supports at most 3 levels: ${item.key}`);
    }
    return item;
  });
}

function requestForTransport(postmanRequest) {
  const request = postmanRequest?.toJSON ? postmanRequest.toJSON() : postmanRequest;
  const headers = (request.header || []).filter((item) => !item.disabled).map((item) => ({
    key: item.key || "",
    value: item.value || "",
    enabled: true,
  }));
  const body = request.body || {};
  let content = "";

  if (body.mode === "raw") {
    content = body.raw || "";
  } else if (body.mode === "urlencoded") {
    content = new URLSearchParams(
      (body.urlencoded || []).filter((item) => !item.disabled).map((item) => [item.key, item.value ?? ""]),
    ).toString();
    if (!headers.some((item) => item.key.toLowerCase() === "content-type")) {
      headers.push({ key: "Content-Type", value: "application/x-www-form-urlencoded", enabled: true });
    }
  } else if (body.mode === "formdata") {
    const boundary = `----Reqly${crypto.randomUUID().replaceAll("-", "")}`;
    content = (body.formdata || []).filter((item) => !item.disabled && item.type !== "file").map((item) =>
      `--${boundary}\r\nContent-Disposition: form-data; name="${String(item.key).replaceAll('"', '%22')}"\r\n\r\n${item.value ?? ""}\r\n`,
    ).join("") + `--${boundary}--\r\n`;
    headers.push({ key: "Content-Type", value: `multipart/form-data; boundary=${boundary}`, enabled: true });
  }

  return {
    method: request.method || "GET",
    url: new PostmanCollection.Url(request.url).toString(),
    headers,
    body: content,
  };
}

const envAlias = `
const __reqlyEnvironmentValues = pm.environment.toObject();
const __reqlyEnvironmentNamespace = (path = []) => new Proxy({}, {
  get: (_, property) => {
    if (typeof property === 'symbol') return undefined;
    const parts = [...path, String(property)];
    const key = parts.join('.');
    if (Object.prototype.hasOwnProperty.call(__reqlyEnvironmentValues, key)) {
      return pm.environment.get(key);
    }
    if (parts.length < 3 && Object.keys(__reqlyEnvironmentValues).some((name) => name.startsWith(key + '.'))) {
      return __reqlyEnvironmentNamespace(parts);
    }
    return undefined;
  },
  set: (_, property, value) => {
    const key = [...path, String(property)].join('.');
    if (key.split('.').length > 3) throw new Error('Environment variable name supports at most 3 levels: ' + key);
    __reqlyEnvironmentValues[key] = value;
    pm.environment.set(key, value);
    return true;
  },
  deleteProperty: (_, property) => {
    const key = [...path, String(property)].join('.');
    delete __reqlyEnvironmentValues[key];
    pm.environment.unset(key);
    return true;
  }
});
const env = __reqlyEnvironmentNamespace();
`;

function toPostmanResponse(response) {
  if (!response) return undefined;
  return {
    code: response.status,
    status: response.statusText,
    header: response.headers.map(([key, value]) => ({ key, value })),
    body: response.body,
    responseTime: response.durationMs,
  };
}

function errorMessage(error) {
  if (typeof error === "string") return error;
  if (error?.message && typeof error.message === "string") return error.message;
  try {
    return JSON.stringify(error) || "Unknown sandbox request error";
  } catch {
    return "Unknown sandbox request error";
  }
}

async function runScript({ target, script, request, response, variables, onRequest, onConsole }) {
  const sandbox = await getSandbox();
  const id = crypto.randomUUID();
  const requestEvent = `execution.request.${id}`;

  const consoleHandler = (_cursor, level, ...args) => onConsole?.({ level, args });
  const requestHandler = async (_cursor, _executionId, eventId, postmanRequest) => {
    try {
      const response = await onRequest(requestForTransport(postmanRequest));
      sandbox.dispatch(`execution.response.${id}`, eventId, null, response, {});
    } catch (error) {
      sandbox.dispatch(`execution.response.${id}`, eventId, { message: errorMessage(error) });
    }
  };
  sandbox.on("console", consoleHandler);
  sandbox.on(requestEvent, requestHandler);

  const event = new Event({
    listen: target,
    script: { type: "text/javascript", exec: [envAlias, script || ""] },
  });
  const options = {
    id,
    timeout: 10000,
    cursor: { iteration: 0, cycles: 1 },
    legacy: { _itemId: request.id, _itemName: request.name },
    context: {
      request: toPostmanRequest(request),
      response: toPostmanResponse(response),
      environment: { values: environmentValues(variables) },
      collectionVariables: { values: [] },
      globals: { values: [] },
      _variables: { values: [] },
    },
  };

  try {
    const result = await new Promise((resolve, reject) => {
      sandbox.execute(event, options, (error, execution) => error ? reject(error) : resolve(execution));
    });
    return {
      request: fromPostmanRequest(result, request),
      variables: environmentFromResult(result),
      nextRequest: result.return?.nextRequest,
    };
  } finally {
    sandbox.off("console", consoleHandler);
    sandbox.off(requestEvent, requestHandler);
  }
}

export function runPreRequest(options) {
  return runScript({ ...options, target: "prerequest" });
}

export function runPostResponse(options) {
  return runScript({ ...options, target: "test" });
}
