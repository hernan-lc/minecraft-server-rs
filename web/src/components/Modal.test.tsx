import { fireEvent, render, screen, waitFor } from "@testing-library/preact";
import { describe, expect, it, vi } from "vitest";
import { DialogProvider, Modal, useDialogs } from "./Modal";
import { I18nProvider } from "../i18n";

function Harness({ onAnswer }: { onAnswer: (value: unknown) => void }) {
  const dialogs = useDialogs();

  return (
    <>
      <button
        onClick={() =>
          dialogs
            .confirm({ title: "Delete world?", body: "Cannot be undone", danger: true })
            .then(onAnswer)
        }
      >
        ask
      </button>
      <button
        onClick={() =>
          dialogs.prompt({ title: "Rename", label: "New name", initial: "old" }).then(onAnswer)
        }
      >
        rename
      </button>
      <button
        onClick={() => {
          void dialogs.confirm({ title: "First question" }).then((answer) =>
            onAnswer(["first", answer]),
          );
          void dialogs.confirm({ title: "Second question" }).then((answer) =>
            onAnswer(["second", answer]),
          );
        }}
      >
        queue confirms
      </button>
      <button
        onClick={() => {
          void dialogs.confirm({ title: "Confirm first" }).then((answer) =>
            onAnswer(["confirm", answer]),
          );
          void dialogs.prompt({ title: "Prompt second", label: "Value" }).then((answer) =>
            onAnswer(["prompt", answer]),
          );
        }}
      >
        queue mixed
      </button>
    </>
  );
}

function setup() {
  const onAnswer = vi.fn();
  render(
    <I18nProvider>
      <DialogProvider>
        <Harness onAnswer={onAnswer} />
      </DialogProvider>
    </I18nProvider>,
  );
  return onAnswer;
}

describe("confirm", () => {
  it("resolves true when accepted", async () => {
    const onAnswer = setup();
    fireEvent.click(screen.getByText("ask"));

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    fireEvent.click(screen.getByText("Confirm"));

    await waitFor(() => expect(onAnswer).toHaveBeenCalledWith(true));
  });

  it("resolves false when dismissed, rather than hanging", async () => {
    const onAnswer = setup();
    fireEvent.click(screen.getByText("ask"));
    fireEvent.click(screen.getByText("Cancel"));

    // A dialog that never settles leaves the caller's `await` stuck forever.
    await waitFor(() => expect(onAnswer).toHaveBeenCalledWith(false));
  });

  it("dismisses on Escape", async () => {
    const onAnswer = setup();
    fireEvent.click(screen.getByText("ask"));
    fireEvent.keyDown(document, { key: "Escape" });

    await waitFor(() => expect(onAnswer).toHaveBeenCalledWith(false));
  });

  it("queues concurrent confirms and settles both callers", async () => {
    const onAnswer = setup();
    fireEvent.click(screen.getByText("queue confirms"));

    expect(screen.getByRole("dialog", { name: "First question" })).toBeInTheDocument();
    fireEvent.click(screen.getByText("Confirm"));

    await waitFor(() =>
      expect(screen.getByRole("dialog", { name: "Second question" })).toBeInTheDocument(),
    );
    fireEvent.click(screen.getByText("Cancel"));

    await waitFor(() => {
      expect(onAnswer).toHaveBeenCalledWith(["first", true]);
      expect(onAnswer).toHaveBeenCalledWith(["second", false]);
    });
  });

  it("queues a prompt behind a confirm and settles both callers", async () => {
    const onAnswer = setup();
    fireEvent.click(screen.getByText("queue mixed"));
    fireEvent.click(screen.getByText("Confirm"));

    await waitFor(() =>
      expect(screen.getByRole("dialog", { name: "Prompt second" })).toBeInTheDocument(),
    );
    fireEvent.input(screen.getByRole("textbox"), { target: { value: "answer" } });
    fireEvent.click(screen.getByText("Confirm"));

    await waitFor(() => {
      expect(onAnswer).toHaveBeenCalledWith(["confirm", true]);
      expect(onAnswer).toHaveBeenCalledWith(["prompt", "answer"]);
    });
  });

  it("shows the next queued dialog after the first is dismissed", async () => {
    const onAnswer = setup();
    fireEvent.click(screen.getByText("queue confirms"));
    fireEvent.keyDown(document, { key: "Escape" });

    await waitFor(() =>
      expect(screen.getByRole("dialog", { name: "Second question" })).toBeInTheDocument(),
    );
    fireEvent.keyDown(document, { key: "Escape" });

    await waitFor(() => expect(onAnswer).toHaveBeenCalledTimes(2));
  });
});

describe("prompt", () => {
  it("resolves the entered text", async () => {
    const onAnswer = setup();
    fireEvent.click(screen.getByText("rename"));

    const input = screen.getByDisplayValue("old");
    fireEvent.input(input, { target: { value: "new-name" } });
    fireEvent.click(screen.getByText("Confirm"));

    await waitFor(() => expect(onAnswer).toHaveBeenCalledWith("new-name"));
  });

  it("treats an emptied field as a dismissal", async () => {
    const onAnswer = setup();
    fireEvent.click(screen.getByText("rename"));

    fireEvent.input(screen.getByDisplayValue("old"), { target: { value: "   " } });
    fireEvent.click(screen.getByText("Confirm"));

    // Otherwise a stray Enter renames a file to the empty string.
    await waitFor(() => expect(onAnswer).toHaveBeenCalledWith(null));
  });

  it("resolves null when cancelled", async () => {
    const onAnswer = setup();
    fireEvent.click(screen.getByText("rename"));
    fireEvent.click(screen.getByText("Cancel"));

    await waitFor(() => expect(onAnswer).toHaveBeenCalledWith(null));
  });
});

describe("Modal component", () => {
  it("renders into document.body via portal and dismisses via close button", () => {
    const onClose = vi.fn();
    const { container } = render(
      <I18nProvider>
        <div id="parent-container">
          <Modal title="Create Server Modal" onClose={onClose} width="lg">
            <form id="test-form">
              <input type="text" placeholder="Server Name" />
            </form>
          </Modal>
        </div>
      </I18nProvider>,
    );

    // Modal dialog is portalled to document.body, not inside parent-container
    const dialog = screen.getByRole("dialog");
    expect(dialog).toBeInTheDocument();
    expect(container.querySelector("#parent-container")?.contains(dialog)).toBe(false);
    expect(document.body.contains(dialog)).toBe(true);

    // Close button (X) in header dismisses
    const closeBtn = screen.getByRole("button", { name: "Close" });
    fireEvent.click(closeBtn);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("dismisses on backdrop click but not on dialog content click", () => {
    const onClose = vi.fn();
    render(
      <I18nProvider>
        <Modal title="Portal Modal" onClose={onClose}>
          <p>Modal content</p>
        </Modal>
      </I18nProvider>,
    );

    // Clicking inside the modal dialog does not close it
    fireEvent.click(screen.getByText("Modal content"));
    expect(onClose).not.toHaveBeenCalled();

    // Clicking the backdrop presentation element closes it
    const backdrop = screen.getByRole("presentation");
    fireEvent.click(backdrop);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("focuses the first input control automatically and locks body overflow", () => {
    const onClose = vi.fn();
    const { unmount } = render(
      <I18nProvider>
        <Modal title="Form Modal" onClose={onClose}>
          <input type="text" placeholder="First Field" />
        </Modal>
      </I18nProvider>,
    );

    const input = screen.getByPlaceholderText("First Field");
    expect(document.activeElement).toBe(input);
    expect(document.body.style.overflow).toBe("hidden");

    unmount();
    expect(document.body.style.overflow).toBe("");
  });

  it("does not refocus after a rerender or a new onClose identity", () => {
    const firstClose = vi.fn();
    const { rerender } = render(
      <I18nProvider>
        <Modal title="Stable focus" onClose={firstClose}>
          <input aria-label="First field" />
          <input aria-label="Second field" />
        </Modal>
      </I18nProvider>,
    );

    const second = screen.getByRole("textbox", { name: "Second field" });
    second.focus();
    const secondClose = vi.fn();
    rerender(
      <I18nProvider>
        <Modal title="Stable focus" onClose={secondClose}>
          <input aria-label="First field" />
          <input aria-label="Second field" />
        </Modal>
      </I18nProvider>,
    );

    expect(document.activeElement).toBe(second);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(secondClose).toHaveBeenCalledTimes(1);
    expect(firstClose).not.toHaveBeenCalled();
  });

  it("traps Tab and Shift+Tab inside the dialog", () => {
    render(
      <I18nProvider>
        <Modal title="Focus trap" onClose={vi.fn()}>
          <input aria-label="Dialog field" />
          <button type="button">Last action</button>
        </Modal>
      </I18nProvider>,
    );

    const close = screen.getByRole("button", { name: "Close" });
    const last = screen.getByRole("button", { name: "Last action" });

    last.focus();
    fireEvent.keyDown(document, { key: "Tab" });
    expect(document.activeElement).toBe(close);

    close.focus();
    fireEvent.keyDown(document, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(last);
  });

  it("restores focus to the opener when it closes", () => {
    const opener = document.createElement("button");
    opener.textContent = "Open";
    document.body.append(opener);
    opener.focus();

    const { unmount } = render(
      <I18nProvider>
        <Modal title="Restoring focus" onClose={vi.fn()}>
          <input aria-label="Dialog field" />
        </Modal>
      </I18nProvider>,
    );

    expect(document.activeElement).not.toBe(opener);
    unmount();
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });

  it("keeps focus inside a dialog with no content controls", () => {
    render(
      <I18nProvider>
        <Modal title="Empty dialog" onClose={vi.fn()}>
          <p>Nothing actionable</p>
        </Modal>
      </I18nProvider>,
    );

    const close = screen.getByRole("button", { name: "Close" });
    expect(document.activeElement).toBe(close);
    fireEvent.keyDown(document, { key: "Tab" });
    expect(document.activeElement).toBe(close);
  });
});
