export const locales = ['en-US', 'zh-CN'] as const;

export type Locale = (typeof locales)[number];

export interface LocaleOption {
  locale: Locale;
  label: string;
  nativeLabel: string;
}

export interface Messages {
  app: {
    title: string;
    deviceContext: string;
  };
  language: {
    label: string;
    english: string;
    chinese: string;
  };
  actions: {
    pause: string;
    resume: string;
    export: string;
    clear: string;
    followTail: string;
  };
  stream: {
    running: string;
    paused: string;
    following: string;
  };
  filters: {
    query: string;
    queryPlaceholder: string;
    direction: string;
    interface: string;
    all: string;
    clear: string;
  };
  directions: {
    call: string;
    reply: string;
    oneway: string;
  };
  statuses: {
    ok: string;
    slow: string;
    error: string;
  };
  streamStats: {
    position: string;
    visible: string;
    total: string;
    dropped: string;
  };
  table: {
    ariaLabel: string;
    sequence: string;
    time: string;
    direction: string;
    process: string;
    interface: string;
    method: string;
    dataSize: string;
    state: string;
    flags: string;
    uid: string;
    code: string;
    noEvents: string;
    jumpLatest: string;
    resizeColumn(label: string): string;
  };
  badges: {
    truncated: string;
  };
  inspector: {
    title: string;
    tabs: string;
    close: string;
    seq: string;
    direction: string;
    status: string;
    sequence: string;
    time: string;
    device: string;
    targetHandle: string;
    code: string;
    flags: string;
    interface: string;
    method: string;
    processLabel: string;
    pid: string;
    tid: string;
    uid: string;
    senderPid: string;
    senderEuid: string;
    dataSize: string;
    offsetsSize: string;
    payloadTruncated: string;
    capturedBytes: string;
    latency: string;
    copyPayload: string;
    copyRaw: string;
    resize: string;
    send: string;
    reply: string;
  };
  detail: {
    summary: string;
    payload: string;
    raw: string;
    correlation: string;
    parsed: string;
    caller: string;
    transaction: string;
    self: string;
    noCorrelation: string;
    copied: string;
    copy: string;
    wrap: string;
    yes: string;
    no: string;
    rawHint: string;
    emptyPayload: string;
    payloadEnd: string;
    capturedOf(captured: number, total: number): string;
  };
}
