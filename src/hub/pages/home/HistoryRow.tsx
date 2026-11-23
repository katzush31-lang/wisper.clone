import { Copy } from "lucide-react";
import { api, type HistoryItem } from "../../../shared/api";
import { errorText, timeLabel, useToast } from "../../components";
import { itemText, statusInfo } from "./helpers";

/** One History row: opens the item when clicked; the copy button copies without opening it. */
export default function HistoryRow({ item, onOpen }: { item: HistoryItem; onOpen: () => void }) {
  const toast = useToast();
  const st = statusInfo(item);
  const text = itemText(item);
  return (
    <div className="list-row home-row">
      <button className="home-row-main" onClick={onOpen}>
        <span className="home-time muted">{timeLabel(item.createdAt)}</span>
        <span className="home-text">
          {text ? <span className="home-snippet">{text}</span> : null}
          {st && (
            <span className={`home-chip ${st.tone}`}>
              {st.label}
              {!text && <span className="home-chip-reason"> · {st.reason}</span>}
            </span>
          )}
        </span>
      </button>
      {text && (
        <button
          className="icon-btn home-copy"
          aria-label="Copy this text"
          title="Copy"
          onClick={(e) => {
            e.stopPropagation();
            api
              .copyText(text)
              .then(() => toast("Copied"))
              .catch((err) => toast(errorText(err), "error"));
          }}
        >
          <Copy size={15} />
        </button>
      )}
    </div>
  );
}
