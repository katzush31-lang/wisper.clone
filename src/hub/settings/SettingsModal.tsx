interface Props {
  open: boolean;
  initialSection?: string;
  onClose: () => void;
}

export default function SettingsModal({ open, onClose }: Props) {
  if (!open) return null;
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="card" onClick={(e) => e.stopPropagation()}>
        Settings coming soon.
      </div>
    </div>
  );
}
