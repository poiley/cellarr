'use client';

// A reusable tag chip-input (#tags). Renders the selected tags as removable
// ASCII chips ("#hd ✕") and offers an add control that either picks an existing
// tag or creates a new one by label. Used by the content tag editor and the
// Indexers / Download Clients / Notifications settings forms to scope a config
// by tag (empty = all). SRCL-only: Input, Button, Badge, Text.
//
// The control is purely local state — it surfaces the chosen tag-id set through
// `onChange` and asks its parent to mint a brand-new tag through `onCreate`
// (so the network call + toast live with the owning form). It never PUTs/POSTs
// itself.

import * as React from 'react';

import Input from '@components/Input';
import Button from '@components/Button';
import Badge from '@components/Badge';
import Text from '@components/Text';

import type { Tag } from '@lib/api/types';

export interface TagInputProps {
  /** Every tag the user can choose from (the `/api/v3/tag` catalogue). */
  available: Tag[];
  /** The currently-selected tag ids. */
  value: number[];
  /** Called with the next id set whenever a chip is added or removed. */
  onChange: (next: number[]) => void;
  /**
   * Mint a brand-new tag from a typed label. Resolves to the created tag so the
   * input can select it. Optional: when omitted, only existing tags can be
   * added (the create affordance is hidden).
   */
  onCreate?: (label: string) => Promise<Tag>;
  /** Accessible group label, e.g. "Tags for this movie". */
  label?: string;
  /** Disable all controls while a parent action is in flight. */
  disabled?: boolean;
}

/** Resolve a tag id to its label, falling back to `#<id>` for an unknown id. */
function labelFor(tags: Tag[], id: number): string {
  return tags.find((t) => t.id === id)?.label ?? `#${id}`;
}

const TagInput: React.FC<TagInputProps> = ({
  available: availableProp,
  value,
  onChange,
  onCreate,
  label = 'Tags',
  disabled = false,
}) => {
  const [draft, setDraft] = React.useState('');
  const [busy, setBusy] = React.useState(false);

  // A list endpoint that 404-falls-through to the SPA index (or a not-yet-loaded
  // catalogue) can hand back a non-array; treat anything but an array as empty so
  // the chip input degrades to "existing tags unavailable" rather than crashing.
  const available = React.useMemo(
    () => (Array.isArray(availableProp) ? availableProp : []),
    [availableProp]
  );

  // Tags not yet selected, ranked by label, that match the current draft text —
  // the pick list the add control offers.
  const suggestions = React.useMemo(() => {
    const q = draft.trim().toLowerCase();
    return available
      .filter((t) => !value.includes(t.id))
      .filter((t) => (q ? t.label.toLowerCase().includes(q) : true))
      .sort((a, b) => a.label.localeCompare(b.label));
  }, [available, value, draft]);

  // An exact (case-insensitive) match for the draft among ALL tags — used to
  // decide whether "Add" selects an existing tag or mints a new one.
  const exact = React.useMemo(() => {
    const q = draft.trim().toLowerCase();
    if (!q) return undefined;
    return available.find((t) => t.label.toLowerCase() === q);
  }, [available, draft]);

  const add = (id: number) => {
    if (!value.includes(id)) onChange([...value, id]);
  };

  const remove = (id: number) => onChange(value.filter((v) => v !== id));

  // The "Add" button: select the exact-match tag if one exists; otherwise mint a
  // new tag via the parent and select it. A blank/whitespace draft is a no-op.
  const commitDraft = async () => {
    const text = draft.trim();
    if (!text || disabled || busy) return;
    if (exact) {
      add(exact.id);
      setDraft('');
      return;
    }
    if (!onCreate) return;
    setBusy(true);
    try {
      const tag = await onCreate(text);
      add(tag.id);
      setDraft('');
    } finally {
      setBusy(false);
    }
  };

  const addLabel = exact ? `Add tag ${exact.label}` : `Add tag ${draft.trim()}`;
  const canAdd = draft.trim().length > 0 && (Boolean(exact) || Boolean(onCreate));

  return (
    <div role="group" aria-label={label}>
      {/* Selected chips: each is a removable "#label ✕" pill. */}
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: '0.5ch', margin: '0.5ch 0' }}>
        {value.length === 0 ? (
          <Text style={{ opacity: 0.4, fontStyle: 'italic' }}>No tags — applies everywhere.</Text>
        ) : (
          value.map((id) => {
            const text = labelFor(available, id);
            return (
              <Badge key={id}>
                #{text}{' '}
                <button
                  type="button"
                  aria-label={`Remove tag ${text}`}
                  disabled={disabled}
                  onClick={() => remove(id)}
                  style={{
                    background: 'none',
                    border: 'none',
                    color: 'inherit',
                    cursor: disabled ? 'default' : 'pointer',
                    font: 'inherit',
                    padding: 0,
                    margin: 0,
                  }}
                >
                  ✕
                </button>
              </Badge>
            );
          })
        )}
      </div>

      {/* Add control: type to filter/create, Enter or the button commits. */}
      <div style={{ display: 'flex', gap: '0.5ch', alignItems: 'stretch' }}>
        <div style={{ flex: 1 }}>
          <Input
            name="tag-input-draft"
            aria-label={`Add a tag to ${label}`}
            placeholder={onCreate ? 'Pick or type a new tag' : 'Pick a tag'}
            value={draft}
            disabled={disabled || busy}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e: React.KeyboardEvent) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                void commitDraft();
              }
            }}
          />
        </div>
        <Button
          theme="SECONDARY"
          aria-label={addLabel}
          isDisabled={disabled || busy || !canAdd}
          onClick={canAdd ? () => void commitDraft() : undefined}
        >
          {busy ? 'Adding…' : exact || !onCreate ? '+ add' : '+ new'}
        </Button>
      </div>

      {/* Existing-tag suggestions: one-click chips for the matching catalogue. */}
      {suggestions.length > 0 ? (
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: '0.5ch', marginTop: '0.5ch' }}>
          {suggestions.slice(0, 12).map((t) => (
            <Button
              key={t.id}
              theme="SECONDARY"
              aria-label={`Add tag ${t.label}`}
              isDisabled={disabled || busy}
              onClick={() => {
                add(t.id);
                setDraft('');
              }}
            >
              #{t.label}
            </Button>
          ))}
        </div>
      ) : null}
    </div>
  );
};

export default TagInput;
