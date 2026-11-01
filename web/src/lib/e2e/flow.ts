/*
 * Hub-level flow control on a channel (`droppen-proto/src/flow.rs`): a sender may
 * have `INITIAL_CREDIT` bytes of `data` payload outstanding per direction; the
 * receiver hands credit back, half a window at a time, as soon as it has read
 * what arrived; the hub enforces both and closes a channel that cheats.
 */
import { INITIAL_CREDIT } from "./frame";

export class SendWindow {
  private available = INITIAL_CREDIT;

  get remaining(): number {
    return this.available;
  }

  canSend(len: number): boolean {
    return len <= this.available;
  }

  consume(len: number): void {
    if (len > this.available) throw new Error(`${len} bytes sent with only ${this.available} bytes of credit`);
    this.available -= len;
  }

  /** The receiver handed `bytes` back; a grant past the window is a protocol error. */
  grant(bytes: number): void {
    if (this.available + bytes > INITIAL_CREDIT) throw new Error(`a grant of ${bytes} bytes would take the window past ${INITIAL_CREDIT}`);
    this.available += bytes;
  }
}

export class RecvWindow {
  private unacked = 0;

  /** `len` bytes were read; the credit to send back now, if any. */
  received(len: number): number | null {
    this.unacked += len;
    if (this.unacked < INITIAL_CREDIT / 2) return null;
    const grant = Math.min(this.unacked, INITIAL_CREDIT);
    this.unacked -= grant;
    return grant;
  }
}
