import { invoke } from '@tauri-apps/api/core';

export type ActivityType =
  | 'file_edited'
  | 'search'
  | 'document_added'
  | 'task_completed'
  | 'command'
  | 'project_switch'
  | 'git_commit';

interface TrackActivityParams {
  activityType: ActivityType;
  data?: string;
  project?: string;
}

/**
 * Hook to track user activities for the timeline
 */
export const useActivityTracker = () => {
  const trackActivity = async ({ activityType, data, project }: TrackActivityParams) => {
    try {
      await invoke('track_activity', {
        activityType,
        data,
        project: project || 'shodh',
      });
    } catch (error) {
      console.error('Failed to track activity:', error);
    }
  };

  return { trackActivity };
};
