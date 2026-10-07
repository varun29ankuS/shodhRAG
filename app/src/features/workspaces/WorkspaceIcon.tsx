import React from 'react';
import {
  BookOpen,
  Briefcase,
  ClipboardCheck,
  FileText,
  FlaskConical,
  Folder,
  GraduationCap,
  Landmark,
  Lightbulb,
  Microscope,
  PenLine,
  Scale,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { cn } from '../../lib/utils';
import { colorClass } from './model';

const ICONS: Record<string, LucideIcon> = {
  folder: Folder,
  'book-open': BookOpen,
  flask: FlaskConical,
  'file-text': FileText,
  'pen-line': PenLine,
  briefcase: Briefcase,
  scale: Scale,
  'graduation-cap': GraduationCap,
  landmark: Landmark,
  lightbulb: Lightbulb,
  microscope: Microscope,
  'clipboard-check': ClipboardCheck,
};

/** A workspace's icon in its colour (decorative: the name is always next to it). */
export function WorkspaceIcon({ icon, color, className }: { icon: string; color: string; className?: string }) {
  const Icon = ICONS[icon] ?? Folder;
  return <Icon className={cn('shrink-0', colorClass(color), className)} aria-hidden="true" />;
}
