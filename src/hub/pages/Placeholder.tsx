interface Props {
  title: string;
  blurb: string;
}

export default function Placeholder({ title, blurb }: Props) {
  return (
    <div className="page page-narrow">
      <div className="page-header">
        <h1 className="page-title">{title}</h1>
      </div>
      <div className="card empty">{blurb}</div>
    </div>
  );
}
