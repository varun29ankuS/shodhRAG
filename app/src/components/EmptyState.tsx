import React from 'react';
import { useTheme } from '../contexts/ThemeContext';
import { LucideIcon } from 'lucide-react';
import { Button } from './ui/button';

interface EmptyStateAction {
  label: string;
  onClick: () => void;
  variant?: 'default' | 'outline' | 'ghost';
  icon?: LucideIcon;
}

interface EmptyStateProps {
  icon?: LucideIcon;
  title: string;
  description?: string;
  suggestions?: string[];
  actions?: EmptyStateAction[];
  size?: 'sm' | 'md' | 'lg';
  variant?: 'default' | 'info' | 'success' | 'warning';
}

export function EmptyState({
  icon: Icon,
  title,
  description,
  suggestions = [],
  actions = [],
  size = 'md',
  variant = 'default',
}: EmptyStateProps) {
  const { colors } = useTheme();

  const sizeConfig = {
    sm: {
      container: 'py-8',
      icon: 'w-8 h-8',
      title: 'text-base',
      description: 'text-xs',
      spacing: 'gap-3',
    },
    md: {
      container: 'py-12',
      icon: 'w-12 h-12',
      title: 'text-lg',
      description: 'text-sm',
      spacing: 'gap-4',
    },
    lg: {
      container: 'py-16',
      icon: 'w-16 h-16',
      title: 'text-xl',
      description: 'text-base',
      spacing: 'gap-6',
    },
  };

  const variantConfig = {
    default: {
      iconColor: colors.textMuted,
      bgColor: 'transparent',
      borderColor: 'transparent',
    },
    info: {
      iconColor: colors.primary,
      bgColor: `${colors.primary}08`,
      borderColor: `${colors.primary}20`,
    },
    success: {
      iconColor: colors.success,
      bgColor: `${colors.success}08`,
      borderColor: `${colors.success}20`,
    },
    warning: {
      iconColor: colors.warning,
      bgColor: `${colors.warning}08`,
      borderColor: `${colors.warning}20`,
    },
  };

  const config = sizeConfig[size];
  const variantStyle = variantConfig[variant];

  return (
    <div
      className={`text-center ${config.container} px-6 rounded-xl border-2 transition-all`}
      style={{
        backgroundColor: variantStyle.bgColor,
        borderColor: variantStyle.borderColor,
      }}
    >
      <div className={`flex flex-col items-center ${config.spacing}`}>
        {Icon && (
          <div className="flex items-center justify-center">
            <Icon className={config.icon} style={{ color: variantStyle.iconColor }} />
          </div>
        )}

        <div className="space-y-2">
          <h3 className={`font-semibold ${config.title}`} style={{ color: colors.text }}>
            {title}
          </h3>

          {description && (
            <p className={config.description} style={{ color: colors.textSecondary }}>
              {description}
            </p>
          )}
        </div>

        {suggestions.length > 0 && (
          <div className="w-full max-w-md space-y-2">
            <p className="text-xs font-medium" style={{ color: colors.textMuted }}>
              Try these:
            </p>
            <div className="space-y-1.5">
              {suggestions.map((suggestion, idx) => (
                <div
                  key={idx}
                  className="flex items-center gap-2 text-left p-2 rounded-lg text-xs"
                  style={{ background: colors.bgSecondary }}
                >
                  <div
                    className="w-1.5 h-1.5 rounded-full flex-shrink-0"
                    style={{ background: colors.primary }}
                  />
                  <span style={{ color: colors.text }}>{suggestion}</span>
                </div>
              ))}
            </div>
          </div>
        )}

        {actions.length > 0 && (
          <div className="flex flex-wrap gap-2 justify-center">
            {actions.map((action, idx) => (
              <Button
                key={idx}
                variant={action.variant || 'default'}
                size="sm"
                onClick={action.onClick}
              >
                {action.icon && <action.icon className="w-4 h-4 mr-2" />}
                {action.label}
              </Button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

interface EmptyStateGridProps {
  icon: LucideIcon;
  title: string;
  items: Array<{
    icon: LucideIcon;
    label: string;
    description: string;
    onClick: () => void;
  }>;
}

export function EmptyStateGrid({ icon: Icon, title, items }: EmptyStateGridProps) {
  const { colors } = useTheme();

  return (
    <div className="text-center py-12 px-6">
      <Icon className="w-12 h-12 mx-auto mb-4" style={{ color: colors.textMuted }} />
      <h3 className="text-lg font-semibold mb-6" style={{ color: colors.text }}>
        {title}
      </h3>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-3 max-w-2xl mx-auto">
        {items.map((item, idx) => (
          <button
            key={idx}
            onClick={item.onClick}
            className="p-4 rounded-lg border-2 text-left hover:scale-105 transition-all group"
            style={{
              background: colors.bgSecondary,
              borderColor: colors.border,
            }}
          >
            <div className="flex items-start gap-3">
              <div
                className="p-2 rounded-lg transition-colors"
                style={{
                  background: colors.bgTertiary,
                }}
              >
                <item.icon className="w-5 h-5" style={{ color: colors.primary }} />
              </div>
              <div className="flex-1">
                <p className="font-semibold text-sm mb-1" style={{ color: colors.text }}>
                  {item.label}
                </p>
                <p className="text-xs" style={{ color: colors.textSecondary }}>
                  {item.description}
                </p>
              </div>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}
