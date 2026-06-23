'use client';

// The expanded body of a single decision-log record: parsed fields, the CF-score
// breakdown, the on-disk comparison, and the raw record. Composed exclusively
// from vendored SRCL primitives.

import * as React from 'react';

import Badge from '@components/Badge';
import CodeBlock from '@components/CodeBlock';
import Divider from '@components/Divider';
import MessageViewer from '@components/MessageViewer';
import Row from '@components/Row';
import Table from '@components/Table';
import TableColumn from '@components/TableColumn';
import TableRow from '@components/TableRow';
import Text from '@components/Text';

import type {
  Decision,
  ParsedRelease,
  Release,
  Score,
  TypedDecisionRecord,
  Verdict,
} from '@app/_lib/decisionlog';
import {
  contentRefLabel,
  formatBytes,
  formatConfidence,
  formatSigned,
  prettyJson,
  rejectReasonLabel,
} from '@app/_lib/decisionlog';

const SectionHeading: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <Text style={{ opacity: 0.6, marginTop: '1ch', marginBottom: '0.5ch', textTransform: 'uppercase', letterSpacing: '0.1ch' }}>
    {children}
  </Text>
);

const KeyValueTable: React.FC<{ rows: [string, React.ReactNode][] }> = ({ rows }) => (
  <Table>
    {rows.map(([label, value]) => (
      <TableRow key={label}>
        <TableColumn style={{ opacity: 0.7, width: '24ch' }}>{label}</TableColumn>
        <TableColumn>{value}</TableColumn>
      </TableRow>
    ))}
  </Table>
);

function parsedRows(parsed: ParsedRelease): [string, React.ReactNode][] {
  const conf = parsed.confidence ?? {};
  const withConfidence = (field: string, value: React.ReactNode): React.ReactNode => {
    const c = conf[field];
    if (value === undefined || value === null || value === '') return '—';
    if (c === undefined) return value;
    return (
      <Row style={{ gap: '1ch', alignItems: 'center' }}>
        <span>{value}</span>
        <Badge>{formatConfidence(c)}</Badge>
      </Row>
    );
  };
  const rows: [string, React.ReactNode][] = [];
  rows.push(['Raw title', parsed.raw_title || '—']);
  if (parsed.clean_title) rows.push(['Clean title', parsed.clean_title]);
  if (parsed.resolution) rows.push(['Resolution', withConfidence('resolution', parsed.resolution)]);
  if (parsed.source) rows.push(['Source', withConfidence('source', parsed.source)]);
  if (parsed.codec) rows.push(['Codec', withConfidence('codec', parsed.codec)]);
  if (parsed.audio && parsed.audio.length) rows.push(['Audio', withConfidence('audio', parsed.audio.join(', '))]);
  if (parsed.hdr && parsed.hdr.length) rows.push(['HDR', withConfidence('hdr', parsed.hdr.join(', '))]);
  if (parsed.edition) rows.push(['Edition', withConfidence('edition', parsed.edition)]);
  if (parsed.languages && parsed.languages.length)
    rows.push(['Languages', withConfidence('languages', parsed.languages.join(', '))]);
  if (parsed.group) rows.push(['Group', withConfidence('group', parsed.group)]);
  if (parsed.proper_repack) rows.push(['Proper / repack', withConfidence('proper_repack', parsed.proper_repack)]);
  if (parsed.year !== undefined) rows.push(['Year', withConfidence('year', parsed.year)]);
  return rows;
}

function releaseRows(release: Release): [string, React.ReactNode][] {
  const rows: [string, React.ReactNode][] = [
    ['Title', release.title],
    ['Protocol', release.protocol],
    ['Size', formatBytes(release.size)],
  ];
  if (release.seeders !== undefined) rows.push(['Seeders', release.seeders]);
  if (release.indexer_flags && release.indexer_flags.length)
    rows.push(['Indexer flags', release.indexer_flags.join(', ')]);
  if (release.guid) rows.push(['GUID', release.guid]);
  return rows;
}

const ScoreTable: React.FC<{ score: Score }> = ({ score }) => (
  <KeyValueTable
    rows={[
      ['Quality rank', `#${score.quality_rank}`],
      ['Custom-format score', formatSigned(score.custom_format_score)],
    ]}
  />
);

const UpgradeComparison: React.FC<{ from: Score; to: Score; replacing: string }> = ({ from, to, replacing }) => {
  const cfDelta = to.custom_format_score - from.custom_format_score;
  const rankDelta = to.quality_rank - from.quality_rank;
  return (
    <Table>
      <TableRow>
        <TableColumn style={{ opacity: 0.7, width: '24ch' }} />
        <TableColumn style={{ opacity: 0.7 }}>On disk</TableColumn>
        <TableColumn style={{ opacity: 0.7 }}>Candidate</TableColumn>
        <TableColumn style={{ opacity: 0.7 }}>Δ</TableColumn>
      </TableRow>
      <TableRow>
        <TableColumn style={{ opacity: 0.7 }}>Quality rank</TableColumn>
        <TableColumn>{`#${from.quality_rank}`}</TableColumn>
        <TableColumn>{`#${to.quality_rank}`}</TableColumn>
        <TableColumn>
          <Badge>{formatSigned(rankDelta)}</Badge>
        </TableColumn>
      </TableRow>
      <TableRow>
        <TableColumn style={{ opacity: 0.7 }}>Custom-format score</TableColumn>
        <TableColumn>{formatSigned(from.custom_format_score)}</TableColumn>
        <TableColumn>{formatSigned(to.custom_format_score)}</TableColumn>
        <TableColumn>
          <Badge>{formatSigned(cfDelta)}</Badge>
        </TableColumn>
      </TableRow>
      <TableRow>
        <TableColumn style={{ opacity: 0.7 }}>Replacing file</TableColumn>
        <TableColumn>{replacing}</TableColumn>
        <TableColumn />
        <TableColumn />
      </TableRow>
    </Table>
  );
};

const VerdictSection: React.FC<{ verdict: Verdict }> = ({ verdict }) => {
  switch (verdict.verdict) {
    case 'grab':
      return (
        <>
          <SectionHeading>CF-score breakdown</SectionHeading>
          <ScoreTable score={verdict.score} />
          <SectionHeading>On-disk comparison</SectionHeading>
          <MessageViewer>Nothing acceptable on disk yet — grabbing to fill the gap.</MessageViewer>
        </>
      );
    case 'upgrade':
      return (
        <>
          <SectionHeading>CF-score breakdown (upgrade)</SectionHeading>
          <UpgradeComparison from={verdict.from} to={verdict.to} replacing={verdict.replacing} />
          <SectionHeading>On-disk comparison</SectionHeading>
          <MessageViewer>
            {`Candidate beats the file on disk by ${formatSigned(
              verdict.to.custom_format_score - verdict.from.custom_format_score
            )} CF — replacing ${verdict.replacing}.`}
          </MessageViewer>
        </>
      );
    case 'reject':
      return (
        <>
          <SectionHeading>Why it was rejected</SectionHeading>
          <MessageViewer>{rejectReasonLabel(verdict.reason)}</MessageViewer>
        </>
      );
  }
};

const DecisionBody: React.FC<{ decision: Decision }> = ({ decision }) => {
  const parsed = decision.parsed;
  return (
    <>
      <SectionHeading>Content</SectionHeading>
      <KeyValueTable
        rows={[
          ['Content', contentRefLabel(decision.content_ref)],
          ['Library', decision.content_ref.library_id],
          ['Coordinates', <CodeBlock key="coords">{prettyJson(decision.content_ref.coords)}</CodeBlock>],
        ]}
      />

      <SectionHeading>Release</SectionHeading>
      <KeyValueTable rows={releaseRows(decision.release)} />

      {parsed ? (
        <>
          <SectionHeading>Parsed fields</SectionHeading>
          <KeyValueTable rows={parsedRows(parsed)} />
        </>
      ) : null}

      <VerdictSection verdict={decision.verdict} />
    </>
  );
};

const DecisionDetail: React.FC<{ record: TypedDecisionRecord }> = ({ record }) => (
  <div style={{ width: '100%' }}>
    {record.decision ? (
      <DecisionBody decision={record.decision} />
    ) : (
      <>
        <SectionHeading>Transition</SectionHeading>
        <MessageViewer>
          {record.note ? record.note : 'This transition carried no decision (a non-Decide stage move).'}
        </MessageViewer>
      </>
    )}

    <Divider type="GRADIENT" style={{ marginTop: '1ch' }} />
    <SectionHeading>Raw record</SectionHeading>
    <CodeBlock>{prettyJson(record)}</CodeBlock>
  </div>
);

export default DecisionDetail;
